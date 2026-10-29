//  SpatiKeyGuard.swift — SpatiKey's Secure Enclave key and Touch ID, in Spatiand.app itself.
//
//  SpatiKey's service (crates/spatiand-mac-core/src/key.rs; the design is docs/spatikey.md) runs in
//  the app from launch, whether or not the room is up, and asks this file for what only the app can
//  do: open the vault key with a finger, wrap it to a new Secure Enclave key, say whether Touch ID
//  is there, and ask the owner about a device that wants to use this Mac. The Touch ID panel names
//  the program that asks, and it has to say Spatiand.
//
//  The key is not in the keychain, which would need entitlements a self-signed app cannot have: it
//  is CryptoKit's dataRepresentation, a blob only this Mac's Secure Enclave can use, which the
//  service keeps in local/spatikey/touch-id.json. Its access control is Touch ID alone, for the
//  fingers enrolled when it was made: no password stands in for a finger, and adding or removing
//  one retires the key until the recovery code sets Touch ID up again.
//
//  The vault key is wrapped to it by ECIES, all here: an ephemeral P-256 key, ECDH with the Secure
//  Enclave key, HKDF-SHA256, AES-256-GCM. Wrapping needs only the public half, so making a key asks
//  nobody anything; unwrapping is an ECDH inside the Secure Enclave, and that is what asks for the
//  finger.
//
//  Whatever waits is off the main thread, the owner's alert excepted. A buffer that holds the vault
//  key is overwritten once used: with Swift's copies on write that means owning the one buffer,
//  which is what `Wiped` is for.

import AppKit
import CryptoKit
import CryptoTokenKit
import CSpatiand
import Foundation
import LocalAuthentication
import Security

/// The wrap of docs/spatikey.md, "Keys", whoever holds the private key: the Secure Enclave in the
/// app, a software key in a test of the format. The same code either way.
enum SpatiKeyWrap {
    static let salt = Data("spatikey".utf8)
    static let associated = Data("spatikey touch-id v1".utf8)
    /// The ephemeral public key (X9.63, 65 bytes), the nonce (12), the wrapped vault key (32), the
    /// tag (16).
    static let length = 65 + 12 + 32 + 16

    /// Not a wrap of this format: the wrong length, or not a point on the curve.
    struct Malformed: Error {}

    /// The 32-byte `secret` wrapped to `recipient`.
    static func wrap(_ secret: Data, to recipient: P256.KeyAgreement.PublicKey) throws -> Data {
        guard secret.count == 32 else { throw Malformed() }
        let ephemeral = P256.KeyAgreement.PrivateKey()
        let shared = try ephemeral.sharedSecretFromKeyAgreement(with: recipient)
        let key = derive(shared, ephemeral: ephemeral.publicKey, recipient: recipient)
        let sealed = try AES.GCM.seal(secret, using: key, authenticating: associated)
        var out = ephemeral.publicKey.x963Representation
        out.append(contentsOf: sealed.nonce)
        out.append(sealed.ciphertext)
        out.append(sealed.tag)
        return out
    }

    /// The secret back. `agree` is the ECDH with the recipient's private key: in the app it runs
    /// inside the Secure Enclave, which is where the finger is asked for.
    static func unwrap(_ wrapped: Data, recipient: P256.KeyAgreement.PublicKey,
                       agree: (P256.KeyAgreement.PublicKey) throws -> SharedSecret) throws -> Data {
        let (ephemeral, sealed) = try parts(wrapped)
        let shared = try agree(ephemeral)
        let key = derive(shared, ephemeral: ephemeral, recipient: recipient)
        return try AES.GCM.open(sealed, using: key, authenticating: associated)
    }

    /// The ephemeral key and the sealed vault key. Read before anything is asked of the Secure
    /// Enclave, so that a damaged file is refused without asking for a finger.
    static func parts(_ wrapped: Data) throws -> (P256.KeyAgreement.PublicKey, AES.GCM.SealedBox) {
        guard wrapped.count == length else { throw Malformed() }
        let bytes = [UInt8](wrapped)
        guard let ephemeral = try? P256.KeyAgreement.PublicKey(x963Representation: bytes[0..<65]) else {
            throw Malformed()
        }
        let sealed = try AES.GCM.SealedBox(nonce: AES.GCM.Nonce(data: bytes[65..<77]),
                                           ciphertext: bytes[77..<109], tag: bytes[109...])
        return (ephemeral, sealed)
    }

    /// HKDF-SHA256 of the shared secret: salt "spatikey", and as its info the ephemeral public key
    /// and then the recipient's, both X9.63.
    private static func derive(_ shared: SharedSecret, ephemeral: P256.KeyAgreement.PublicKey,
                               recipient: P256.KeyAgreement.PublicKey) -> SymmetricKey {
        shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: salt,
                                       sharedInfo: ephemeral.x963Representation + recipient.x963Representation,
                                       outputByteCount: 32)
    }
}

final class SpatiKeyGuard {
    static let shared = SpatiKeyGuard()

    /// Why not, in the service's words -- "denied", "touch_id_unavailable", "fingerprints_changed",
    /// "timeout", "failed" -- and in the owner's.
    struct Refusal: Error, Equatable {
        let code: String
        let message: String
    }

    /// Where a failed unlock failed: restoring the key from its blob, the ECDH inside the Secure
    /// Enclave, or opening what that unwrapped.
    enum Stage { case restore, agree, open }

    /// A failed unlock, before it is put in the service's words.
    struct Failure: Error {
        let error: Error
        let stage: Stage
        /// How long it took: a person who was asked takes seconds, a key refused outright none.
        let seconds: TimeInterval
    }

    /// The service gives up on a finger after 120 s and on the owner after 180 s. The panel and the
    /// alert go a little before, so that nothing is answered that nobody hears.
    static let touchWait: TimeInterval = 110
    static let allowWait: TimeInterval = 170
    /// The most one touch covers, whatever is asked: SpatiKey offers 60 and 300 seconds.
    static let longestLinger: TimeInterval = 300

    /// Every use of the Secure Enclave key, one at a time: a second unlock waits for the first
    /// finger rather than raising a panel over the first one.
    private let queue = DispatchQueue(label: "spatiand.spatikey", qos: .userInitiated)
    /// The owner's answers about devices, one alert at a time.
    private let alerts = DispatchQueue(label: "spatiand.spatikey.alerts", qos: .userInitiated)
    /// One touch covering what follows: the context the finger was given to, and until when.
    /// Only on `queue`.
    private var lingering: (context: LAContext, until: Date)?
    private var started = false

    // MARK: starting

    /// Start SpatiKey's service: at launch, once.
    func start() {
        guard !started else { return }
        started = true
        let dir = NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
        let begin = {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            let name = Host.current().localizedName ?? "Mac"
            let serving = sp_key_start(dir, name, nil, SpatiKeyGuard.asked) != 0
            print(serving ? "SpatiKey: the service is running" : "SpatiKey: the service could not start")
        }
        if FileManager.default.fileExists(atPath: dir + "/identity.key") {
            // Off the main thread, and off `queue`: the service's first steps may take a moment, and
            // nothing it asks should wait behind them.
            DispatchQueue.global(qos: .utility).async(execute: begin)
        } else {
            // The very first launch makes this Mac's identity, which the host and the link make too
            // if it is not there: here, in turn with them, rather than two of them writing it at once.
            begin()
        }
        // Its socket goes with the app, and a question still waiting is refused rather than left.
        _ = NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification, object: nil,
                                                   queue: nil) { _ in sp_key_stop() }
    }

    // MARK: what the service asks

    /// What the service asks, on one of its threads: the question is copied, worked on elsewhere,
    /// and answered with sp_key_answer.
    static let asked: sp_key_ask_fn = { _, id, what, json in
        guard let json else {
            return SpatiKeyGuard.answer(id, SpatiKeyGuard.refused(Refusal(code: "failed", message: "SpatiKey asked nothing")))
        }
        SpatiKeyGuard.shared.asked(Int(what), id: id, question: Wiped(copying: json), at: Date())
    }

    private func asked(_ what: Int, id: UInt64, question: Wiped, at asked: Date) {
        switch what {
        case SP_KEY_UNLOCK:
            queue.async { self.answerUnlock(id, question, asked: asked) }
        case SP_KEY_ENROLL:
            queue.async { self.answerEnroll(id, question) }
        case SP_KEY_CAN_TOUCH:
            // Not behind a finger being waited for: it asks nobody, and the service wants it now.
            DispatchQueue.global(qos: .userInitiated).async { Self.answer(id, ["ok": Self.canTouch()]) }
        case SP_KEY_ALLOW:
            alerts.async { self.answerAllow(id, question, asked: asked) }
        default:
            Self.answer(id, Self.refused(Refusal(code: "failed", message: "SpatiKey asked the app for something it does not know (\(what))")))
        }
    }

    private func answerUnlock(_ id: UInt64, _ question: Wiped, asked: Date) {
        guard let object = question.object(),
              let blob = (object["blob"] as? String).flatMap({ Data(base64Encoded: $0) }),
              let wrapped = (object["wrapped"] as? String).flatMap({ Data(base64Encoded: $0) }) else {
            return Self.answer(id, Self.refused(Refusal(code: "failed", message: "SpatiKey's question for Touch ID could not be read")))
        }
        // Waited behind another finger so long that the service will soon stop listening: better no
        // panel than one whose answer goes nowhere.
        let left = Self.touchWait - Date().timeIntervalSince(asked)
        guard left > 10 else {
            return Self.answer(id, Self.refused(Refusal(code: "timeout", message: "Touch ID was busy with another request")))
        }
        do {
            var vk = try unlock(reason: object["reason"] as? String ?? "", blob: blob, wrapped: wrapped,
                                linger: (object["linger"] as? NSNumber)?.doubleValue ?? 0, within: left)
            defer { vk.resetBytes(in: vk.startIndex..<vk.endIndex) }
            Self.answerKey(id, vk)
        } catch {
            let refusal = error as? Refusal
                ?? Refusal(code: "failed", message: "Touch ID could not open the vault key (\(Self.detail(error)))")
            print("SpatiKey: Touch ID did not open the vault key: \(refusal.code)")
            Self.answer(id, Self.refused(refusal))
        }
    }

    private func answerEnroll(_ id: UInt64, _ question: Wiped) {
        guard var vk = question.secret("vk") else {
            return Self.answer(id, Self.refused(Refusal(code: "failed", message: "SpatiKey's question carried no vault key")))
        }
        defer { vk.resetBytes(in: vk.startIndex..<vk.endIndex) }
        guard vk.count == 32 else {
            return Self.answer(id, Self.refused(Refusal(code: "failed", message: "SpatiKey's question carried no vault key")))
        }
        do {
            let made = try enroll(vk: vk)
            Self.answer(id, ["public": made.public.base64EncodedString(), "blob": made.blob.base64EncodedString(),
                             "wrapped": made.wrapped.base64EncodedString()])
        } catch {
            let refusal = error as? Refusal
                ?? Refusal(code: "failed", message: "Touch ID could not be set up (\(Self.detail(error)))")
            print("SpatiKey: Touch ID was not set up: \(refusal.code)")
            Self.answer(id, Self.refused(refusal))
        }
    }

    private func answerAllow(_ id: UInt64, _ question: Wiped, asked: Date) {
        let object = question.object() ?? [:]
        let code = object["code"] as? String ?? ""
        // Waited behind another alert so long that the service will soon stop listening.
        let left = Self.allowWait - Date().timeIntervalSince(asked)
        guard left > 10, !code.isEmpty else { return Self.answer(id, ["allow": false]) }
        let allow = allowDevice(name: object["name"] as? String ?? "", fp: object["fp"] as? String ?? "",
                                code: code, within: left)
        Self.answer(id, ["allow": allow])
    }

    // MARK: the four answers

    /// The vault key from `wrapped`, with a finger on Touch ID; the panel says "Spatiand is trying to
    /// <reason>". With `linger` above 0 the touch covers what follows for that many seconds; at 0
    /// every use asks. Gives up after `within` seconds. Throws a `Refusal`.
    func unlock(reason: String, blob: Data, wrapped: Data, linger: TimeInterval,
                within: TimeInterval = touchWait) throws -> Data {
        guard (try? SpatiKeyWrap.parts(wrapped)) != nil else {
            throw Refusal(code: "failed", message: "SpatiKey's Touch ID file is damaged: the recovery code sets Touch ID up again")
        }
        let linger = min(max(linger, 0), Self.longestLinger)
        let sentence = Self.sentence(reason)
        var context: LAContext
        var reused = false
        if linger > 0, let kept = lingering, kept.until > Date() {
            context = kept.context
            reused = true
        } else {
            forget()
            context = Self.freshContext()
        }
        while true {
            do {
                let vk = try attempt(context, sentence: sentence, blob: blob, wrapped: wrapped, within: within)
                if linger == 0 {
                    context.invalidate()
                } else if !reused {
                    keep(context, for: linger)
                }
                return vk
            } catch {
                // A context that failed is not kept, whichever it was.
                if reused { forget() } else { context.invalidate() }
                // The kept touch had run out after all: one fresh one is asked for.
                if reused, let failure = error as? Failure, Self.contextGone(failure.error) {
                    context = Self.freshContext()
                    reused = false
                    continue
                }
                if let failure = error as? Failure { throw Self.refusal(for: failure) }
                throw error
            }
        }
    }

    /// One try: restore the key with `context`, then the ECDH -- which asks for the finger, unless
    /// `context` already has one -- and open what it unwraps.
    private func attempt(_ context: LAContext, sentence: String, blob: Data, wrapped: Data,
                         within: TimeInterval) throws -> Data {
        context.localizedReason = sentence
        // Taken down unanswered before the service stops waiting.
        let late = Flag()
        let deadline = DispatchWorkItem {
            late.set()
            context.invalidate()
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + within, execute: deadline)
        defer { deadline.cancel() }
        let began = Date()
        var stage = Stage.restore
        do {
            let key = try SecureEnclave.P256.KeyAgreement.PrivateKey(dataRepresentation: blob, authenticationContext: context)
            stage = .agree
            return try SpatiKeyWrap.unwrap(wrapped, recipient: key.publicKey) { ephemeral in
                let shared = try key.sharedSecretFromKeyAgreement(with: ephemeral)
                stage = .open
                return shared
            }
        } catch {
            if late.isSet { throw Refusal(code: "timeout", message: "no finger was put on Touch ID in time") }
            throw Failure(error: error, stage: stage, seconds: Date().timeIntervalSince(began))
        }
    }

    /// A new Secure Enclave key for this Mac, for the fingers enrolled now, and `vk` wrapped to it.
    /// Nothing is asked of anyone: the key is made, not used, and wrapping needs only its public
    /// half. Throws a `Refusal`.
    func enroll(vk: Data) throws -> (public: Data, blob: Data, wrapped: Data) {
        guard SecureEnclave.isAvailable, Self.canTouch() else {
            throw Refusal(code: "touch_id_unavailable",
                          message: "Touch ID cannot be used now, so SpatiKey cannot be set up with it: is the lid shut, or no finger enrolled?")
        }
        var problem: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(kCFAllocatorDefault, kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
                                                            [.privateKeyUsage, .biometryCurrentSet], &problem) else {
            let error = problem?.takeRetainedValue()
            throw Refusal(code: "failed", message: "no access control for Touch ID (\(error.map { Self.detail($0) } ?? "?"))")
        }
        // A context that may not show anything: were making or restoring the key ever to want a
        // finger, it would fail here instead of asking.
        let quiet = LAContext()
        quiet.interactionNotAllowed = true
        do {
            let key = try SecureEnclave.P256.KeyAgreement.PrivateKey(accessControl: access, authenticationContext: quiet)
            // The blob is all the service keeps: it has to give this key back.
            let again = try SecureEnclave.P256.KeyAgreement.PrivateKey(dataRepresentation: key.dataRepresentation,
                                                                        authenticationContext: quiet)
            guard again.publicKey.x963Representation == key.publicKey.x963Representation else {
                throw Refusal(code: "failed", message: "the Secure Enclave did not give its new key back")
            }
            let wrapped = try SpatiKeyWrap.wrap(vk, to: key.publicKey)
            return (key.publicKey.x963Representation, key.dataRepresentation, wrapped)
        } catch let refusal as Refusal {
            throw refusal
        } catch {
            throw Refusal(code: "failed", message: "the Secure Enclave would not make a key (\(Self.detail(error)))")
        }
    }

    /// Touch ID is there, has a finger enrolled, and is not locked out. Asks without any panel.
    static func canTouch() -> Bool {
        var error: NSError?
        return LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    }

    /// The owner's yes or no to a device that wants to ask this Mac, with the six digits both show.
    /// The safe answer is the default button: the alert takes the keyboard, and a Return typed for
    /// something else must not let a device in.
    func allowDevice(name: String, fp: String, code: String, within: TimeInterval = allowWait) -> Bool {
        print("SpatiKey: \(Self.shown(name)) (\(fp.prefix(8))) asks to use this Mac")
        let ask = { () -> Bool in
            NSApp.activate(ignoringOtherApps: true)
            let alert = NSAlert()
            alert.messageText = "\u{201C}\(Self.shown(name))\u{201D} wants to use SpatiKey on this Mac"
            alert.informativeText = "Make sure it shows \(Self.spaced(code)). Each use still needs your finger here."
            let allow = alert.addButton(withTitle: "Allow")
            let refuse = alert.addButton(withTitle: "Don\u{2019}t Allow")
            allow.keyEquivalent = ""
            refuse.keyEquivalent = "\r"
            // Taken down unanswered before the service stops waiting: a yes nobody hears lets nothing in.
            let timer = Timer(timeInterval: within, repeats: false) { _ in NSApp.abortModal() }
            RunLoop.main.add(timer, forMode: .modalPanel)
            defer { timer.invalidate() }
            return alert.runModal() == .alertFirstButtonReturn
        }
        return Thread.isMainThread ? ask() : DispatchQueue.main.sync(execute: ask)
    }

    // MARK: a touch that covers what follows

    /// Let this touch cover what follows for `seconds`, and then let it go.
    private func keep(_ context: LAContext, for seconds: TimeInterval) {
        lingering = (context, Date().addingTimeInterval(seconds))
        queue.asyncAfter(deadline: .now() + seconds) { [weak self] in
            if let self, self.lingering?.context === context { self.forget() }
        }
    }

    /// No touch covers anything any more.
    private func forget() {
        lingering?.context.invalidate()
        lingering = nil
    }

    /// A context for one finger: Touch ID and nothing else, no "Use Password..." button.
    private static func freshContext() -> LAContext {
        let context = LAContext()
        context.localizedFallbackTitle = ""
        return context
    }

    // MARK: what went wrong

    /// A failed unlock in the service's words. The codes that say so come first: cancelled, or Touch
    /// ID not usable now, in whichever domain they arrive. Past those, Touch ID works and the key
    /// itself was refused, which is what adding or removing a finger does to a key made for the
    /// fingers of then -- unless the key worked and only what it unwrapped did not open, or the
    /// panel was up long enough for a finger that did not match: the Security framework and
    /// CryptoTokenKit say that with the same code as a retired key when LocalAuthentication's own
    /// error is not inside theirs, and the time is all that tells the two apart.
    static func refusal(for failure: Failure, touchAvailable: () -> Bool = canTouch) -> Refusal {
        let errors = chain(failure.error)
        let detail = Self.detail(failure.error)
        func has(_ domain: String, _ codes: Int...) -> Bool {
            errors.contains { $0.domain == domain && codes.contains($0.code) }
        }
        let la = LAError.errorDomain, security = NSOSStatusErrorDomain, token = TKError.errorDomain
        if has(la, LAError.Code.userCancel.rawValue, LAError.Code.appCancel.rawValue, LAError.Code.systemCancel.rawValue,
               LAError.Code.userFallback.rawValue)
            || has(security, Int(errSecUserCanceled)) || has(token, TKError.Code.canceledByUser.rawValue) {
            return Refusal(code: "denied", message: "Touch ID was cancelled")
        }
        if has(la, LAError.Code.authenticationFailed.rawValue) {
            return Refusal(code: "denied", message: "the finger on Touch ID did not match")
        }
        if has(la, LAError.Code.biometryLockout.rawValue) {
            return Refusal(code: "touch_id_unavailable",
                           message: "Touch ID is locked after too many tries: unlock this Mac with its password once, then try again")
        }
        if has(la, LAError.Code.biometryNotEnrolled.rawValue) {
            return Refusal(code: "touch_id_unavailable", message: "no finger is enrolled in Touch ID")
        }
        // -11 is "no companion device", whose name is newer than this app's oldest macOS.
        if has(la, LAError.Code.biometryNotAvailable.rawValue, LAError.Code.biometryNotPaired.rawValue,
               LAError.Code.biometryDisconnected.rawValue, LAError.Code.passcodeNotSet.rawValue, -11) {
            return Refusal(code: "touch_id_unavailable", message: "Touch ID is not available now: is the lid shut? (\(detail))")
        }
        if has(la, LAError.Code.notInteractive.rawValue) || has(security, Int(errSecInteractionNotAllowed))
            || has(token, TKError.Code.authenticationNeeded.rawValue) {
            return Refusal(code: "touch_id_unavailable", message: "Touch ID cannot ask now: is this Mac locked? (\(detail))")
        }
        if !touchAvailable() {
            return Refusal(code: "touch_id_unavailable", message: "Touch ID cannot be used now (\(detail))")
        }
        if failure.stage == .open {
            return Refusal(code: "failed",
                           message: "Touch ID opened its key, but the vault key in SpatiKey's file did not open (\(detail))")
        }
        if failure.seconds >= 2,
           has(security, Int(errSecAuthFailed)) || has(token, TKError.Code.authenticationFailed.rawValue) {
            return Refusal(code: "denied", message: "the finger on Touch ID did not match")
        }
        return Refusal(code: "fingerprints_changed",
                       message: "the fingers enrolled in Touch ID changed since SpatiKey was set up on this Mac: the recovery code sets it up again (\(detail))")
    }

    /// The kept context is no good any more.
    private static func contextGone(_ error: Error) -> Bool {
        chain(error).contains { $0.domain == LAError.errorDomain && $0.code == LAError.Code.invalidContext.rawValue }
    }

    /// An error and the ones inside it: the Secure Enclave's arrive wrapped, LocalAuthentication's
    /// inside the Security framework's or CryptoTokenKit's.
    private static func chain(_ error: Error) -> [NSError] {
        var all: [NSError] = []
        var next: NSError? = error as NSError
        while let error = next, all.count < 8 {
            all.append(error)
            next = error.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return all
    }

    /// An error's domain and number, for the log and the message: never its text, which can be long.
    static func detail(_ error: Error) -> String {
        let error = error as NSError
        return "\(error.domain) \(error.code)"
    }

    // MARK: words

    /// What finishes "Spatiand is trying to ...": one line, not too long, never empty.
    static func sentence(_ reason: String) -> String {
        let line = reason.components(separatedBy: .controlCharacters).joined(separator: " ").trimmingCharacters(in: .whitespaces)
        if line.isEmpty { return "open SpatiKey" }
        return line.count > 160 ? String(line.prefix(159)) + "\u{2026}" : line
    }

    /// A device's name as it gives it, fit for one line of an alert.
    static func shown(_ name: String) -> String {
        let line = name.components(separatedBy: .controlCharacters).joined(separator: " ").trimmingCharacters(in: .whitespaces)
        if line.isEmpty { return "A device" }
        return line.count > 40 ? String(line.prefix(39)) + "\u{2026}" : line
    }

    /// Six digits as both ends show them: "123 456".
    static func spaced(_ code: String) -> String {
        let digits = code.filter { ("0"..."9").contains($0) }
        guard digits.count == 6 else { return code }
        return "\(digits.prefix(3)) \(digits.suffix(3))"
    }

    // MARK: answering

    private static func answer(_ id: UInt64, _ object: [String: Any]) {
        let data = (try? JSONSerialization.data(withJSONObject: object)) ?? Data(#"{"error":"failed","message":""}"#.utf8)
        sp_key_answer(id, String(decoding: data, as: UTF8.self))
    }

    private static func refused(_ refusal: Refusal) -> [String: Any] {
        ["error": refusal.code, "message": refusal.message]
    }

    /// `{"vk": base64}`, written by hand into a buffer that is overwritten once the service has its
    /// own copy.
    private static func answerKey(_ id: UInt64, _ vk: Data) {
        var encoded = vk.base64EncodedData()
        defer { encoded.resetBytes(in: encoded.startIndex..<encoded.endIndex) }
        let head = Array(#"{"vk":""#.utf8), tail = Array(#""}"#.utf8)
        let text = Wiped(count: head.count + encoded.count + tail.count)
        text.fill { out in
            var at = 0
            for byte in head { out[at] = byte; at += 1 }
            for byte in encoded { out[at] = byte; at += 1 }
            for byte in tail { out[at] = byte; at += 1 }
        }
        text.withCString { sp_key_answer(id, $0) }
    }

    /// Bytes that may carry the vault key, overwritten when let go. Copied in once and never copied
    /// out, so that the one buffer there is is the one cleared: a `Data` or an array could have been
    /// copied on a write, leaving the old buffer as it was.
    private final class Wiped {
        private let bytes: UnsafeMutableBufferPointer<UInt8>
        /// Without the NUL after them.
        let count: Int

        init(count: Int) {
            self.count = count
            bytes = .allocate(capacity: count + 1)
            bytes.initialize(repeating: 0)
        }

        /// A copy of a C string.
        convenience init(copying text: UnsafePointer<CChar>) {
            self.init(count: strlen(text))
            UnsafeMutableRawPointer(bytes.baseAddress!).copyMemory(from: text, byteCount: count)
        }

        deinit {
            memset_s(bytes.baseAddress, bytes.count, 0, bytes.count)
            bytes.deallocate()
        }

        func fill(_ write: (UnsafeMutableBufferPointer<UInt8>) -> Void) {
            write(UnsafeMutableBufferPointer(rebasing: bytes[0..<count]))
        }

        func withCString<R>(_ body: (UnsafePointer<CChar>) -> R) -> R {
            body(UnsafeRawPointer(bytes.baseAddress!).assumingMemoryBound(to: CChar.self))
        }

        /// The question as JSON: for a question with no secret in it.
        func object() -> [String: Any]? {
            let data = Data(bytesNoCopy: bytes.baseAddress!, count: count, deallocator: .none)
            return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        }

        /// The base64 string `name` decoded, read straight from the bytes, with no parser's strings
        /// in between that nobody would clear. For what key.rs writes by hand: `{"vk":"..."}`.
        func secret(_ name: String) -> Data? {
            let key = Array("\"\(name)\":\"".utf8)
            var start: Int?
            var at = 0
            while start == nil, at + key.count <= count {
                if (0..<key.count).allSatisfy({ bytes[at + $0] == key[$0] }) { start = at + key.count }
                at += 1
            }
            guard let start else { return nil }
            var end = start
            while end < count, bytes[end] != UInt8(ascii: "\"") { end += 1 }
            guard end < count else { return nil }
            let text = Data(bytesNoCopy: UnsafeMutableRawPointer(bytes.baseAddress! + start), count: end - start,
                            deallocator: .none)
            return Data(base64Encoded: text)
        }
    }

    /// A yes that one thread sets and another reads.
    private final class Flag {
        private let lock = NSLock()
        private var value = false

        func set() {
            lock.lock()
            value = true
            lock.unlock()
        }

        var isSet: Bool {
            lock.lock()
            defer { lock.unlock() }
            return value
        }
    }
}
