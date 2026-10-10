//  MacHost.swift — this Mac, as a computer other devices can use the windows of.
//
//  The same protocol the Linux host speaks, so a Deck or a Beam Pro shows a Mac's windows as it
//  shows any host's. Applications are what is running here; a session that "launches" one is
//  given that application's windows, and any it opens later. Each window is captured by
//  ScreenCaptureKit and encoded by the hardware as HEVC; a window that is not changing sends
//  nothing. The session's mouse, wheel and keys are posted back at the application, its clipboard
//  is this Mac's clipboard, and closing and resizing are done through Accessibility.
//
//  Needs Screen Recording (to capture) and Accessibility (to act). One device at a time, the
//  newest winning, and a device is served nothing until it has been paired.

import AppKit
import ApplicationServices
import CoreMedia
import ScreenCaptureKit

final class MacHost {
    static let shared = MacHost()
    static let port: UInt16 = 47600
    /// The protocol's own version, which a session checks.
    private static let protocolVersion = 3

    private(set) var link: HostLink?
    private let clipboard = ClipboardSync()
    private(set) var attached = false
    /// What the status menu says.
    private(set) var status = "Off"
    var onChange: (() -> Void)?

    /// One window being shared.
    private final class Shared {
        let id: UInt16
        var info: MacWindowInfo
        let capture: MacCapture
        let lock = NSLock()
        var encoder: HostEncoder?
        var lastPixels: CVPixelBuffer?
        var wantsKey = true
        init(id: UInt16, info: MacWindowInfo) {
            self.id = id
            self.info = info
            capture = MacCapture(info)
        }
    }

    private var shared: [UInt16: Shared] = [:]
    /// The sound of each application that has a window being shared, by bundle.
    private var sounds: [String: MacAudio] = [:]
    private var soundFailed: [String: Date] = [:]
    private var nextID: UInt16 = 1
    /// The applications a session has launched: their windows are shared, now and when they open.
    private var launched: Set<String> = []
    private var focus: UInt16?
    private var kbit = 10_000
    private var poll: Timer?
    private var lastCatalog = ""
    private var held: Set<UInt32> = []
    private var pairingTimer: Timer?
    private(set) var pairing = false
    private var lastClick: (at: Date, point: CGPoint, count: Int)?
    private var pressedButton: CGMouseButton = .left

    var running: Bool { link != nil }

    // MARK: starting and stopping

    func start() {
        guard link == nil else { return }
        let dir = NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
        guard let l = HostLink(identityDir: dir, port: Self.port) else {
            status = "Could not listen on port \(Self.port)"
            onChange?()
            return
        }
        link = l
        // **A notification on this Mac goes to whoever is looking at it from a room.**
        Notices.shared.onShown = { [weak self] text in self?.link?.say(["Notice": ["text": text]]) }
        Notices.shared.start()
        l.onJoined = { [unowned self] in joined($0) }
        l.onAttached = { [unowned self] in greet() }
        l.onSaid = { [unowned self] in said($0) }
        l.onLeft = { [unowned self] refused in left(refused) }
        clipboard.send = { [weak l] in l?.say($0) }
        clipboard.start()
        if ProcessInfo.processInfo.environment["SPATIAND_HOST_TRUST_SELF"] != nil { l.trust(l.fingerprint) }
        status = "Waiting for a device"
        let t = Timer(timeInterval: 1, repeats: true) { [weak self] _ in self?.tick() }
        RunLoop.main.add(t, forMode: .common)
        poll = t
        onChange?()
    }

    func stop() {
        poll?.invalidate()
        poll = nil
        pairingTimer?.invalidate()
        pairing = false
        detach()
        link = nil
        status = "Off"
        onChange?()
    }

    /// This host's address as a person would give it to another device.
    var address: String { "\(ProcessInfo.processInfo.hostName):\(Self.port)" }
    var fingerprintShort: String { String((link?.fingerprint ?? "").prefix(8)) }
    var pairedCount: Int { link?.pairedCount ?? 0 }
    func forgetAllDevices() { link?.forgetAll(); onChange?() }

    // MARK: pairing

    /// Let a device that has never been here ask to pair, for two minutes.
    func openPairing() {
        guard let link else { return }
        link.openPairing(true)
        pairing = true
        pairingTimer?.invalidate()
        pairingTimer = Timer.scheduledTimer(withTimeInterval: 120, repeats: false) { [weak self] _ in self?.closePairing() }
        onChange?()
    }

    func closePairing() {
        link?.openPairing(false)
        pairing = false
        pairingTimer?.invalidate()
        onChange?()
    }

    private func joined(_ info: [String: Any]) {
        let known = info["known"] as? Bool ?? false
        let name = info["short"] as? String ?? "a device"
        if known {
            status = "Serving \(name)"
            onChange?()
            return
        }
        // For tests with no one at the Mac to press the button: the environment has to say so.
        if ProcessInfo.processInfo.environment["SPATIAND_HOST_AUTO_ALLOW"] != nil {
            link?.decide(true)
            closePairing()
            return
        }
        let code = info["code"] as? String ?? "??? ???"
        let address = info["address"] as? String ?? ""
        let alert = NSAlert()
        alert.messageText = "A device wants to use this Mac's windows"
        alert.informativeText = "Check that it shows the same six digits:\n\n\(code)\n\nIt is connecting from \(address). Allow it only if you started this and the digits match."
        alert.addButton(withTitle: "Allow")
        alert.addButton(withTitle: "Don't allow")
        NSApp.activate(ignoringOtherApps: true)
        let allowed = alert.runModal() == .alertFirstButtonReturn
        link?.decide(allowed)
        if allowed { closePairing() }
    }

    private func left(_ refused: Bool) {
        detach()
        status = running ? "Waiting for a device" : "Off"
        onChange?()
    }

    // MARK: a session arrives

    private func greet() {
        guard let link else { return }
        attached = true
        status = "In use"
        clipboard.connected = true
        link.say(["Welcome": ["version": Self.protocolVersion, "host": Host.current().localizedName ?? ProcessInfo.processInfo.hostName,
                              "reattached": !launched.isEmpty]])
        sendCatalog(force: true)
        reconcile()
        onChange?()
    }

    /// The session has gone; the applications stay, and so does what it launched.
    private func detach() {
        attached = false
        clipboard.connected = false
        for s in shared.values { s.capture.invalidate(); s.encoder?.invalidate() }
        shared = [:]
        for a in sounds.values { a.invalidate() }
        sounds = [:]
        held = []
        focus = nil
    }

    // MARK: the catalogue

    private func runningApps() -> [NSRunningApplication] {
        NSWorkspace.shared.runningApplications.filter {
            $0.activationPolicy == .regular && $0.bundleIdentifier != nil && $0.processIdentifier != ProcessInfo.processInfo.processIdentifier
        }
    }

    private func icon(of app: NSRunningApplication) -> String? {
        guard let icon = app.icon else { return nil }
        let side = 64
        let image = NSImage(size: NSSize(width: side, height: side))
        image.lockFocus()
        icon.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
        image.unlockFocus()
        guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let png = rep.representation(using: .png, properties: [:]) else { return nil }
        return png.base64EncodedString()
    }

    private func sendCatalog(force: Bool) {
        guard let link, attached else { return }
        let apps = runningApps().sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
        let key = apps.compactMap { $0.bundleIdentifier }.joined(separator: ",")
        guard force || key != lastCatalog else { return }
        lastCatalog = key
        let list: [[String: Any]] = apps.map { app in
            var entry: [String: Any] = ["id": app.bundleIdentifier ?? "", "name": app.localizedName ?? "", "exec": "open"]
            if let png = icon(of: app) { entry["icon_png"] = png }
            return entry
        }
        link.say(["Catalog": ["apps": list]])
    }

    // MARK: the windows

    private func tick() {
        guard attached else { return }
        sendCatalog(force: false)
        reconcile()
    }

    /// Share the windows of every application a session has launched, and stop sharing ones that have gone.
    private func reconcile() {
        guard attached, MacWindows.allowed(ask: false) else { return }
        Task { [weak self] in
            let list = await MacWindows.list()
            await MainActor.run { self?.apply(list) }
        }
    }

    private func apply(_ list: [MacWindowInfo]) {
        guard attached, let link else { return }
        let wanted = list.filter { launched.contains($0.bundle) }
        let byWindow = Dictionary(uniqueKeysWithValues: wanted.map { ($0.windowID, $0) })
        // Gone.
        for (id, s) in shared where byWindow[s.info.windowID] == nil {
            s.capture.invalidate()
            s.encoder?.invalidate()
            shared[id] = nil
            link.say(["Closed": ["window": Int(id)]])
            if focus == id { focus = nil }
        }
        // New, and retitled.
        for info in wanted {
            if let existing = shared.values.first(where: { $0.info.windowID == info.windowID }) {
                if existing.info.title != info.title {
                    existing.info = info
                    link.say(["Retitled": ["window": Int(existing.id), "title": info.title]])
                }
                continue
            }
            share(info)
        }
        // Sound for the applications with a window, and none for the ones that have none left.
        let bundles = Set(shared.values.map { $0.info.bundle })
        for (bundle, a) in sounds where !bundles.contains(bundle) { a.invalidate(); sounds[bundle] = nil }
        for bundle in bundles where sounds[bundle] == nil && Date().timeIntervalSince(soundFailed[bundle] ?? .distantPast) > 30 {
            listen(to: bundle)
        }
    }

    private func share(_ info: MacWindowInfo) {
        guard let link else { return }
        let id = nextID
        nextID = nextID == UInt16.max ? 1 : nextID + 1
        let s = Shared(id: id, info: info)
        shared[id] = s
        let scale = NSScreen.screens.first { $0.frame.intersects(info.frame) }?.backingScaleFactor ?? 2
        link.say(["Opened": ["window": Int(id), "app": info.bundle, "title": info.title,
                             "width": Int(info.frame.width * scale), "height": Int(info.frame.height * scale), "parent": NSNull()]])
        s.capture.onFrame = { [weak self, weak s] pixels, micros in
            guard let self, let s else { return }
            self.frame(s, pixels, micros)
        }
        s.capture.onEnded = { [weak self] in DispatchQueue.main.async { self?.reconcile() } }
        Task { [weak self] in
            do { try await s.capture.start() } catch {
                await MainActor.run {
                    print("host: could not capture \(info.app): \(error)")
                    self?.shared[id] = nil
                    self?.link?.say(["Closed": ["window": Int(id)]])
                }
            }
        }
    }

    private func listen(to bundle: String) {
        let audio = MacAudio(bundle: bundle)
        sounds[bundle] = audio
        audio.onSound = { [weak self] pcm in self?.link?.audio(app: bundle, channels: 2, pcm) }
        audio.onEnded = { [weak self] in
            if self?.sounds[bundle] === audio { self?.sounds[bundle] = nil }
        }
        Task {
            do { try await audio.start() } catch {
                await MainActor.run {
                    print("host: could not capture the sound of \(bundle): \(error)")
                    self.soundFailed[bundle] = Date()
                    if self.sounds[bundle] === audio { self.sounds[bundle] = nil }
                }
            }
        }
    }

    /// A picture of a window, off the main thread: encode it, and say its size first when that changed.
    private func frame(_ s: Shared, _ pixels: CVPixelBuffer, _ micros: UInt64) {
        guard let link else { return }
        s.lock.lock()
        defer { s.lock.unlock() }
        s.lastPixels = pixels
        let w = CVPixelBufferGetWidth(pixels), h = CVPixelBufferGetHeight(pixels)
        if s.encoder == nil || s.encoder?.width != w || s.encoder?.height != h {
            s.encoder?.invalidate()
            let id = s.id
            s.encoder = HostEncoder(width: w, height: h, kbit: perWindowKbit) { data, key, us in
                link.video(window: id, keyframe: key, capturedMicros: us, data)
            }
            if s.encoder == nil { print("host: the encoder would not start at \(w)x\(h)") }
            s.wantsKey = true
            link.say(["Stream": ["window": Int(id), "codec": "H265", "width": w, "height": h, "eyes": "mono"]])
        }
        s.encoder?.encode(pixels, microseconds: micros, key: s.wantsKey)
        s.wantsKey = false
    }

    private var perWindowKbit: Int { max(1_000, kbit / max(1, shared.count)) }

    /// A window that is not changing sends nothing; so when a session asks for a picture, encode the last one again.
    private func resend(_ s: Shared) {
        s.lock.lock()
        defer { s.lock.unlock() }
        s.wantsKey = true
        if let pixels = s.lastPixels, let encoder = s.encoder {
            encoder.encode(pixels, microseconds: UInt64(ProcessInfo.processInfo.systemUptime * 1_000_000), key: true)
            s.wantsKey = false
        }
    }

    // MARK: what the session says

    private func windowNumber(_ fields: [String: Any]) -> UInt16? {
        (fields["window"] as? NSNumber).map { UInt16(truncatingIfNeeded: $0.intValue) }
    }

    private func said(_ message: [String: Any]) {
        guard let (name, body) = message.first else { return }
        let fields = body as? [String: Any] ?? [:]
        if ProcessInfo.processInfo.environment["SPATIAND_DEBUG"] != nil, name != "InputAt" { print("host: session said \(name) \(fields)") }
        switch name {
        case "Launch":
            if let app = fields["app"] as? String { launch(app) }
        case "Close":
            if let id = windowNumber(fields), let s = shared[id] { close(s.info) }
        case "Configure":
            if let id = windowNumber(fields), let s = shared[id], let w = (fields["width"] as? NSNumber)?.doubleValue,
               let h = (fields["height"] as? NSNumber)?.doubleValue {
                resize(s, pixelsWide: w, high: h)
            }
        case "Focus":
            focus = windowNumber(fields)
            if let id = focus, let s = shared[id] { NSRunningApplication(processIdentifier: s.info.pid)?.activate() }
        case "WantKeyframe":
            if let id = windowNumber(fields), let s = shared[id] { resend(s) }
        case "InputAt", "Input":
            if let id = windowNumber(fields), let s = shared[id], let input = fields["input"] { act(on: s, input) }
        case "Clipboard":
            clipboard.fromHost(fields)
        case "Bandwidth":
            if let k = (fields["max_kbit"] as? NSNumber)?.intValue {
                kbit = min(max(k, 2_000), 40_000)
                for s in shared.values { s.lock.lock(); s.encoder?.setBitrate(perWindowKbit); s.lock.unlock() }
            }
        default:
            break
        }
    }

    // MARK: launching, closing, resizing

    private func launch(_ bundle: String) {
        launched.insert(bundle)
        if NSRunningApplication.runningApplications(withBundleIdentifier: bundle).isEmpty,
           let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle) {
            NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration())
        }
        reconcile()
    }

    private func close(_ info: MacWindowInfo) {
        guard let window = MacWindows.axWindow(info) else { return }
        var button: CFTypeRef?
        if AXUIElementCopyAttributeValue(window, kAXCloseButtonAttribute as CFString, &button) == .success, let button {
            AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString)
        }
    }

    /// Once the viewer's drag has stopped; see `SettledResize`.
    private let sizes = SettledResize()

    private func resize(_ s: Shared, pixelsWide: Double, high: Double) {
        let scale = s.capture.scale
        let info = s.info
        sizes.want(UInt32(truncatingIfNeeded: info.windowID)) {
            MacWindows.resize(info, toPoints: CGSize(width: pixelsWide / scale, height: high / scale))
        }
    }

    // MARK: input

    private static let terminals: Set<String> = ["com.apple.Terminal", "com.googlecode.iterm2", "net.kovidgoyal.kitty", "org.alacritty",
                                                 "com.mitchellh.ghostty", "com.github.wez.wezterm", "dev.warp.Warp-Stable"]

    private var controlIsCommand: Bool { Defaults.store.object(forKey: "hostControlIsCommand") as? Bool ?? true }

    private func flags(for app: String) -> CGEventFlags {
        var flags: CGEventFlags = []
        if held.contains(42) || held.contains(54) { flags.insert(.maskShift) }
        if held.contains(56) || held.contains(100) { flags.insert(.maskAlternate) }
        if held.contains(29) || held.contains(97) {
            // A Mac user's copy and paste are Command; on the device they are Control. A terminal
            // keeps Control, which is an interrupt there.
            flags.insert(controlIsCommand && !Self.terminals.contains(app) ? .maskCommand : .maskControl)
        }
        if held.contains(125) || held.contains(126) { flags.insert(.maskCommand) }
        return flags
    }

    private static let macKey: [UInt32: UInt16] = {
        var table: [UInt32: UInt16] = [:]
        for (mac, evdev) in KeyMap.evdev where table[evdev] == nil { table[evdev] = mac }
        return table
    }()

    private func act(on s: Shared, _ input: Any) {
        let info = s.info
        // A message with no fields is its bare name.
        if let name = input as? String {
            if name == "Leave" { return }
            return
        }
        guard let (kind, body) = (input as? [String: Any])?.first, let fields = body as? [String: Any] else { return }
        switch kind {
        case "Motion":
            guard let x = (fields["x"] as? NSNumber)?.doubleValue, let y = (fields["y"] as? NSNumber)?.doubleValue else { return }
            let type: CGEventType = pressedDown ? (pressedButton == .right ? .rightMouseDragged : .leftMouseDragged) : .mouseMoved
            MacInput.mouse(type, button: pressedButton, at: s.capture.screenPoint(x: x, y: y), clicks: 1, window: info.windowID, pid: info.pid)
            lastPoint = (x, y)
        case "Button":
            guard let code = (fields["button"] as? NSNumber)?.intValue, let down = fields["pressed"] as? Bool else { return }
            let button: CGMouseButton = code == 0x111 ? .right : (code == 0x112 ? .center : .left)
            let point = s.capture.screenPoint(x: lastPoint.0, y: lastPoint.1)
            var clicks = 1
            if down {
                if let last = lastClick, Date().timeIntervalSince(last.at) < 0.4, abs(last.point.x - point.x) < 6, abs(last.point.y - point.y) < 6 {
                    clicks = last.count + 1
                }
                lastClick = (Date(), point, clicks)
                pressedButton = button
            } else {
                clicks = lastClick?.count ?? 1
            }
            pressedDown = down
            let type: CGEventType = {
                switch button {
                case .right: return down ? .rightMouseDown : .rightMouseUp
                case .center: return down ? .otherMouseDown : .otherMouseUp
                default: return down ? .leftMouseDown : .leftMouseUp
                }
            }()
            MacInput.mouse(type, button: button, at: point, clicks: clicks, window: info.windowID, pid: info.pid)
        case "Scroll":
            let h = (fields["horizontal"] as? NSNumber)?.doubleValue ?? 0
            let v = (fields["vertical"] as? NSNumber)?.doubleValue ?? 0
            // A notch is a few units and a trackpad's pixels are many; a notch is about forty pixels.
            func pixels(_ d: Double) -> Int32 { Int32(-d * (abs(d) <= 3 ? 40 : 1)) }
            MacInput.scroll(dx: pixels(h), dy: pixels(v), at: s.capture.screenPoint(x: lastPoint.0, y: lastPoint.1), window: info.windowID, pid: info.pid)
        case "Key":
            guard let code = (fields["code"] as? NSNumber)?.uint32Value, let down = fields["pressed"] as? Bool else { return }
            if down { held.insert(code) } else { held.remove(code) }
            // The modifiers ride on the keys that follow; they are not keys of their own here.
            if [42, 54, 29, 97, 56, 100, 125, 126, 58].contains(code) { return }
            guard let mac = Self.macKey[code] else { return }
            MacInput.key(code: mac, flags: NSEvent.ModifierFlags(rawValue: UInt(flags(for: info.bundle).rawValue)), down: down, pid: info.pid)
        default:
            break
        }
    }

    private var pressedDown = false
    private var lastPoint: (Double, Double) = (0, 0)
}
