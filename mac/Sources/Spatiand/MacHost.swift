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
import CSpatiand
import ApplicationServices
import CoreMedia
import ScreenCaptureKit

final class MacHost {
    static let shared = MacHost()
    /// 47600; a test instance beside the real one says another (`SPATIAND_HOST_PORT`).
    static var port: UInt16 { ProcessInfo.processInfo.environment["SPATIAND_HOST_PORT"].flatMap { UInt16($0) } ?? 47600 }
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
        /// This Mac is locked: no picture of it is encoded or sent (under `lock`).
        var paused = false
        init(id: UInt16, info: MacWindowInfo) {
            self.id = id
            self.info = info
            capture = MacCapture(info)
        }
    }

    private var shared: [UInt16: Shared] = [:]

    /// **A screen shown to the session stands for the windows on it**, as in the room
    /// (`RoomWindows`; the owner, 2026-11-11: "when screen is pulled to the room, make it as
    /// native mac, don't override mac behaviour"): a window opened on it is not sent as a
    /// stream of its own beside the screen it is already seen in, and one the session gives the
    /// keys is not made the Mac's active one for that alone -- only when it is pressed or
    /// scrolled in its own picture. One asked for from the list by hand is still shown.
    private func onSharedScreen(_ info: MacWindowInfo) -> Bool {
        guard info.display == nil else { return false }
        let displays = Set(shared.values.compactMap { $0.info.display }.compactMap { MacScreens.current($0) })
        guard !displays.isEmpty else { return false }
        let frame = MacWindows.bounds(info.windowID) ?? info.frame
        var found: CGDirectDisplayID = 0
        var count: UInt32 = 0
        guard CGGetDisplaysWithPoint(CGPoint(x: frame.midX, y: frame.midY), 1, &found, &count) == .success, count > 0 else { return false }
        return displays.contains(found)
    }
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
    /// The code shown while pairing is open, and the panel it is shown in. Shown here only.
    private(set) var pairingCode: String?
    private var pairingPanel: NSPanel?
    private var pairingText: NSTextField?
    private var lastClick: (at: Date, point: CGPoint, count: Int)?
    private var pressedButton: CGMouseButton = .left

    var running: Bool { link != nil }

    // MARK: starting and stopping

    func start() {
        guard link == nil else { return }
        // A test instance keeps an identity and a paired list of its own (`SPATIAND_HOST_DIR`).
        let dir = ProcessInfo.processInfo.environment["SPATIAND_HOST_DIR"] ?? NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
        guard let l = HostLink(identityDir: dir, port: Self.port) else {
            status = "Could not listen on port \(Self.port)"
            onChange?()
            return
        }
        link = l
        // **A notification on this Mac goes to whoever is looking at it from a room.**
        // Read for as long as this Mac is a host, whether or not its own room is in the glasses
        // (`Notices.Reader`); said only to a session that is attached, and not while this Mac is
        // locked, when what it would say is said as a state (`sayHeld`).
        Notices.shared.onShown = { [weak self] text in
            guard let self, self.attached, !self.macLocked, text.isEmpty || Settings.notifications else { return }
            if !text.isEmpty { print("host: a notification here, said to the session") }
            self.link?.say(["Notice": ["text": text]])
        }
        // A banner the session pressed (`NoticePress`) opened an application: its windows are
        // the session's to see, as one it had started.
        Notices.shared.onOpened = { [weak self] bundle in
            guard let self, self.attached, self.pressedNotice != nil else { return }
            self.pressedNotice = nil
            print("host: the session pressed a notification; showing it the windows of \(bundle)")
            self.launched.insert(bundle)
            self.reconcile()
        }
        Notices.shared.start(for: .host)
        // (Asked now, not when it is first said: locked, Touch ID answers no.)
        _ = Self.unlockWords
        l.onJoined = { [unowned self] in joined($0) }
        l.onAttached = { [unowned self] in greet() }
        l.onSaid = { [unowned self] in said($0) }
        l.onLeft = { [unowned self] refused in left(refused) }
        l.onPairing = { [unowned self] in pairingNews($0) }
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
        Notices.shared.stop(for: .host)
        poll?.invalidate()
        poll = nil
        closePairing()
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

    /// Let a device that has never been here pair, for two minutes: a fresh code, shown here
    /// and only here, for the person to type on the device. A device that proves it is paired;
    /// nobody is asked to allow it (crates/spatiand-stream/src/pairing.rs).
    func openPairing() {
        guard let link, let code = link.openPairing(true) else { return }
        pairing = true
        pairingCode = code
        pairingTimer?.invalidate()
        pairingTimer = Timer.scheduledTimer(withTimeInterval: 120, repeats: false) { [weak self] _ in self?.closePairing() }
        // Printed for `--serve-host`, where whoever runs it is at this terminal; never logged.
        if CommandLine.arguments.contains("--serve-host") { print("pairing code \(code)") }
        showPairing("On the device, add this Mac (\(address)) and type the code\n\n\(Self.grouped(code))\n\nOnly a device that types it is paired. Keep it to yourself: a paired device can use this Mac's windows and ask its SpatiKey.")
        onChange?()
    }

    func closePairing() {
        link?.openPairing(false)
        pairing = false
        pairingCode = nil
        pairingTimer?.invalidate()
        pairingPanel?.close()
        pairingPanel = nil
        pairingText = nil
        onChange?()
    }

    private static func grouped(_ code: String) -> String {
        code.count == 6 ? "\(code.prefix(3)) \(code.suffix(3))" : code
    }

    /// The code, in a panel that stays up while pairing is open and does not stop anything else.
    private func showPairing(_ text: String) {
        if pairingPanel == nil {
            let label = NSTextField(wrappingLabelWithString: "")
            label.font = .systemFont(ofSize: 15)
            label.frame = NSRect(x: 20, y: 20, width: 380, height: 170)
            let panel = NSPanel(
                contentRect: NSRect(x: 0, y: 0, width: 420, height: 210),
                styleMask: [.titled, .utilityWindow], backing: .buffered, defer: false)
            panel.title = "Pairing a device"
            panel.contentView?.addSubview(label)
            panel.center()
            panel.level = .floating
            pairingPanel = panel
            pairingText = label
        }
        pairingText?.stringValue = text
        NSApp.activate(ignoringOtherApps: true)
        pairingPanel?.makeKeyAndOrderFront(nil)
    }

    /// What became of an attempt at the code.
    private func pairingNews(_ news: [String: Any]) {
        if let who = news["paired"] as? String {
            closePairing()
            status = "Paired with \(who)"
            PairingUI.alert("Paired", "The device \(who) typed the code, and can use this Mac from now on.")
        } else if let wrong = news["wrong"] as? [String: Any] {
            let who = wrong["who"] as? String ?? "a device"
            let left = wrong["left"] as? Int ?? 0
            if let code = pairingCode {
                showPairing("A device (\(who)) did not give the right code; \(left) tries left.\n\nOn the device, type the code\n\n\(Self.grouped(code))")
            }
        } else if news["locked"] != nil {
            closePairing()
            PairingUI.alert("Pairing closed", "Three wrong codes were given, so pairing closed. Nothing was written down.")
        }
        onChange?()
    }

    private func joined(_ info: [String: Any]) {
        let known = info["known"] as? Bool ?? false
        let name = info["short"] as? String ?? "a device"
        status = known ? "Serving \(name)" : "A device is proving the code"
        onChange?()
    }

    private func left(_ refused: Bool) {
        settingsPeer = false
        detach()
        status = running ? "Waiting for a device" : "Off"
        onChange?()
    }

    // MARK: a session arrives

    private func greet() {
        guard let link else { return }
        attached = true
        openAtAttach = nil
        openedSince = []
        passedOver = []
        offered = [:]
        askedFor = []
        status = "In use"
        clipboard.connected = true
        link.say(["Welcome": ["version": Self.protocolVersion, "host": Host.current().localizedName ?? ProcessInfo.processInfo.hostName,
                              "reattached": !launched.isEmpty]])
        sendCatalog(force: true)
        // Locked: said as a state before anything else is, so the session's launcher and card
        // say what is waited for from the start.
        toldLocked = false
        if macLocked { sayHeld() }
        reconcile()
        onChange?()
    }

    /// The session has gone; the applications stay, and so does what it launched.
    private func detach() {
        attached = false
        // The next session is sent the catalogue whatever this one was sent.
        lastCatalog = ""
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

    /// What is installed on this Mac, looked at again when an application folder may have
    /// changed (every half minute, and whenever what is running changes).
    private var installed: [Room.Installed] = []
    private var installedLooked = Date.distantPast
    private var catalogBusy = false
    private var catalogAgain = false
    private var lastRunning = ""

    /// **Every application this Mac can start, not only the ones running** (the owner,
    /// 2026-11-17, of a Deck's launcher that showed Claude and Finder and nothing else:
    /// "currently, only active app shown on deck"). The same list the Mac's own room's launcher
    /// has (`Room.applications`), by name, each under its bundle's identifier, which is what
    /// `Launch` names and `launch` opens; and whatever is running from somewhere else (an
    /// application run from a download folder), so that its windows can still be asked for.
    /// The list and its icons are read off the main thread: eighty bundles and their pictures.
    private func sendCatalog(force: Bool) {
        guard link != nil, attached else { return }
        // **One asked for while a listing is under way is done when it ends**: a session that
        // attaches again then was otherwise never sent the catalogue (its launcher stayed empty).
        guard !catalogBusy else { if force { catalogAgain = true }; return }
        let running = runningApps()
        let runningKey = running.compactMap { $0.bundleIdentifier }.sorted().joined(separator: ",")
        let stale = Date().timeIntervalSince(installedLooked) > 30
        guard force || stale || runningKey != lastRunning else { return }
        lastRunning = runningKey
        let extra: [(id: String, name: String, icon: String?)] = running.compactMap { app in
            guard let id = app.bundleIdentifier else { return nil }
            return (id, app.localizedName ?? id, nil)
        }
        let known = installed
        catalogBusy = true
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let apps = force || stale || known.isEmpty ? Room.applications() : known
            var seen: Set<String> = []
            var list: [[String: Any]] = []
            for app in apps {
                guard let id = app.bundle, seen.insert(id).inserted else { continue }
                var entry: [String: Any] = ["id": id, "name": app.title, "exec": "open"]
                if let file = app.icon, let png = FileManager.default.contents(atPath: file) { entry["icon_png"] = png.base64EncodedString() }
                list.append(entry)
            }
            DispatchQueue.main.async {
                guard let self else { return }
                self.catalogBusy = false
                self.installed = apps
                if force || stale || known.isEmpty { self.installedLooked = Date() }
                // Running, and not in an application folder: after the others, by name.
                for app in extra.sorted(by: { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }) where seen.insert(app.id).inserted {
                    var entry: [String: Any] = ["id": app.id, "name": app.name, "exec": "open"]
                    if let found = running.first(where: { $0.bundleIdentifier == app.id }), let png = self.icon(of: found) { entry["icon_png"] = png }
                    list.append(entry)
                }
                let key = list.compactMap { $0["id"] as? String }.joined(separator: ",")
                if self.attached, force || key != self.lastCatalog {
                    self.lastCatalog = key
                    print("host: \(list.count) applications offered to the session")
                    self.link?.say(["Catalog": ["apps": list]])
                }
                if self.catalogAgain {
                    self.catalogAgain = false
                    self.sendCatalog(force: true)
                }
            }
        }
    }

    // MARK: the windows

    /// The windows that were open when the session attached, those opened since, and those that
    /// are not windows in their own right.
    private var openAtAttach: Set<CGWindowID>?
    private var openedSince: Set<CGWindowID> = []
    private var passedOver: Set<CGWindowID> = []
    /// Windows offered to the session and not yet asked for, by the number each was offered
    /// under; and the ones it has asked for.
    private var offered: [UInt16: MacWindowInfo] = [:]
    private var askedFor: Set<CGWindowID> = []

    /// The owner's list, kept with the hosts this Mac can use: a host paired from any of their
    /// devices can be used from here, and one removed from any cannot.
    private var rosterLooked = Date.distantPast
    /// What changed in the settings, told by the Mac's own link to a host.
    func tellConfig(_ reply: Any) {
        if attached, settingsPeer { link?.say(["ConfigSync": reply]) }
    }

    private func followRoster() {
        guard Date().timeIntervalSince(rosterLooked) >= 2, let link else { return }
        rosterLooked = Date()
        let me = link.fingerprint
        DispatchQueue.global(qos: .utility).async {
            // Where it can be reached too: a host nobody has dialled yet is otherwise on every device's list with no address.
            sp_roster_note_host(me, Host.current().localizedName ?? "Mac", ProcessInfo.processInfo.hostName, Self.port)
            for host in Hosts.all { sp_roster_note(host.fingerprint, host.name, host.address, 0) }
            guard let raw = sp_roster_json() else { return }
            defer { sp_settings_free(raw) }
            let entries = ((try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))) as? [[String: Any]]) ?? []
            DispatchQueue.main.async {
                for entry in entries {
                    guard let fp = entry["fp"] as? String, fp != me else { continue }
                    let removed = (entry["removed_ms"] as? NSNumber)?.intValue ?? 0 != 0
                    let known = Hosts.all.first { $0.fingerprint.lowercased() == fp.lowercased() }
                    if removed, let known { Hosts.forget(known) }
                    let address = entry["address"] as? String ?? ""
                    if !removed, known == nil, !address.isEmpty {
                        let name = entry["name"] as? String ?? ""
                        Hosts.add(PairedHost(name: name.isEmpty ? address : name, address: address, fingerprint: fp))
                    }
                }
            }
        }
    }

    private func tick() {
        followRoster()
        guard attached else { return }
        // (Not one on a screen the session is shown: there it can be reached wherever it is put.)
        for s in shared.values where s.info.display == nil && !onSharedScreen(s.info) {
            MacWindows.bringOntoScreen(s.info, frame: s.capture.frame())
        }
        sendCatalog(force: false)
        reconcile()
        // What changed in the settings here, every few seconds, to a session that keeps them.
        if settingsPeer, !Model.shared.connected, Date().timeIntervalSince(settingsLooked) >= 3 {
            settingsLooked = Date()
            sayAll(settingsReplies(sp_settings_changed()))
        }
    }

    /// Share the windows of every application a session has launched, and stop sharing ones that have gone.
    /// Whether this Mac is locked. **macOS lets nothing capture its windows or make a screen while
    /// it is** (its own security, not to be got round), so a locked Mac offers nothing and says why,
    /// rather than opening each window, failing and opening it again every second -- which a
    /// session sees as windows that blink and never show. Set by the app (`locked(_:)` in main).
    var macLocked = false {
        didSet {
            guard macLocked != oldValue else { return }
            toldLocked = false
            // **Locked, nothing more of this Mac is sent**: no picture of a window already
            // shown (each paused, a key picture asked for at the unlock) and no sound (taken
            // again by `reconcile` once it is unlocked).
            for s in shared.values {
                let paused = macLocked
                // (Off the main thread, which must never wait for a window's lock; on one queue,
                // so that a lock and an unlock close together are taken in their order.)
                pauseQueue.async { s.lock.lock(); s.paused = paused; s.wantsKey = true; s.lock.unlock() }
            }
            if macLocked {
                for a in sounds.values { a.invalidate() }
                sounds = [:]
                sayHeld()
            } else {
                // **Said, and at once**: whoever unlocked it did so with a finger on this Mac's
                // sensor, and the session they came from showed nothing of it having been taken.
                print("host: this Mac is unlocked; the session is told, and shown what it asked for meanwhile")
                if attached { link?.say(["Held": "No"]) }
                reconcile()
            }
        }
    }
    private var toldLocked = false
    private let pauseQueue = DispatchQueue(label: "host-pause")

    /// How this Mac is unlocked, for the session to say: with a finger where it has a sensor a
    /// finger is enrolled on. Asked once, before it is ever locked (locked, the answer is no).
    private static let unlockWords: String = SpatiKeyGuard.canTouch() ? "Touch ID or its password" : "its password"

    /// This Mac is locked: the session is told so as a state (`Held`), which it shows for as
    /// long as it lasts and says the end of, and in a notice as well, which is all a session
    /// built before `Held` understands (one that knows `Held` passes the notice over).
    private func sayHeld() {
        guard attached, let link else { return }
        toldLocked = true
        print("host: this Mac is locked; the session is told its windows wait until it is unlocked")
        link.say(["Held": ["Locked": ["unlock": Self.unlockWords]]])
        link.say(["Notice": ["text": "\(Host.current().localizedName ?? "This Mac") is locked: unlock it to use its windows here."]])
    }

    /// Whether the session has pressed a notification's card and what that opens is awaited.
    private var pressedNotice: UUID?

    /// Windows whose capture macOS refused, and when: not asked for again on their own until a
    /// while has passed, as the sound's (`soundFailed`).
    private var captureFailed: [CGWindowID: Date] = [:]

    private func reconcile() {
        guard attached, MacWindows.allowed(ask: false) else { return }
        if macLocked {
            if !toldLocked { sayHeld() }
            return
        }
        Task { [weak self] in
            let list = await MacWindows.list()
            await MainActor.run { self?.apply(list) }
        }
    }

    private func apply(_ windows: [MacWindowInfo]) {
        // (A listing asked for just before this Mac was locked shares nothing new.)
        guard attached, !macLocked, let link else { return }
        // This Mac's screens with its windows: each a thing that can be shown, and none of
        // them shown until it is asked for.
        let screens = MacScreens.list(excluding: nil)
        let list = windows + screens
        // **What was open when the session attached waits to be asked for; what is opened while
        // it is attached is shared at once**, whichever application's it is -- as in the room.
        let here = Set(list.map(\.windowID))
        if openAtAttach == nil {
            openAtAttach = here
            // **What is already open on this Mac is offered**: in the session's list of windows,
            // put away, each to be asked for. Nothing of them is captured or sent until then.
            for info in windows where !launched.contains(info.bundle) && MacWindows.isOrdinary(info) {
                let id = nextID
                nextID = nextID == UInt16.max ? 1 : nextID + 1
                offered[id] = info
                link.say(["Offered": ["window": Int(id), "title": info.title.isEmpty ? info.app : info.app + " \u{2014} " + info.title]])
            }
        }
        // A screen is offered whenever it is neither on offer nor being shown: when the session
        // attaches, when a display is plugged in, and again when one that was shown is closed.
        for info in screens where !askedFor.contains(info.windowID) && !offered.values.contains(where: { $0.windowID == info.windowID })
            && !shared.values.contains(where: { $0.info.windowID == info.windowID }) {
            let id = nextID
            nextID = nextID == UInt16.max ? 1 : nextID + 1
            offered[id] = info
            link.say(["OfferedScreen": ["window": Int(id), "title": info.title]])
        }
        // An offered window that has closed is off the list.
        for (id, info) in offered where !here.contains(info.windowID) {
            offered[id] = nil
            link.say(["Closed": ["window": Int(id)]])
        }
        for info in windows where !(openAtAttach ?? []).contains(info.windowID) && !openedSince.contains(info.windowID) && !passedOver.contains(info.windowID) {
            if MacWindows.isOrdinary(info) { openedSince.insert(info.windowID) } else { passedOver.insert(info.windowID) }
        }
        openedSince.formIntersection(here)
        passedOver.formIntersection(here)
        let refusedLately = { (id: CGWindowID) in Date().timeIntervalSince(self.captureFailed[id] ?? .distantPast) < 30 }
        let wanted = list.filter {
            askedFor.contains($0.windowID)
                || !refusedLately($0.windowID) && ((launched.contains($0.bundle) && !passedOver.contains($0.windowID)) || openedSince.contains($0.windowID))
                    // On a screen the session is shown, it is seen there already.
                    && !onSharedScreen($0)
        }
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
        // A screen's sound is everything but the applications that have streams of their own:
        // taken again when which those are changes.
        let apart = bundles.subtracting([MacScreens.bundle])
        if let screen = sounds[MacScreens.bundle], apart != screenSoundApart { screen.invalidate(); sounds[MacScreens.bundle] = nil }
        screenSoundApart = apart
        for (bundle, a) in sounds where !bundles.contains(bundle) { a.invalidate(); sounds[bundle] = nil }
        for bundle in bundles where sounds[bundle] == nil && Date().timeIntervalSince(soundFailed[bundle] ?? .distantPast) > 30 {
            listen(to: bundle)
        }
    }

    private func share(_ info: MacWindowInfo) {
        guard let link else { return }
        // Under the number it was offered with, if it was: that is the one the session knows.
        let id: UInt16
        if let was = offered.first(where: { $0.value.windowID == info.windowID })?.key {
            id = was
            offered[was] = nil
        } else {
            id = nextID
            nextID = nextID == UInt16.max ? 1 : nextID + 1
        }
        let s = Shared(id: id, info: info)
        shared[id] = s
        // A screen's picture is in its own pixels, which are not always two to a point.
        // **A screen is sent at the size it looks**, not at the size macOS draws it before
        // scaling it down: 1792 across and not 3584. That is what is seen on it, at a
        // quarter of the pixels to encode, send and decode.
        s.capture.pointsOnly = info.display != nil
        let scale = info.display != nil ? 1 : NSScreen.screens.first { $0.frame.intersects(info.frame) }?.backingScaleFactor ?? 2
        link.say(["Opened": ["window": Int(id), "app": info.bundle, "title": info.display != nil ? "Screen \u{2014} " + info.title : info.title,
                             "width": Int(info.frame.width * scale), "height": Int(info.frame.height * scale), "parent": NSNull()]])
        // **Its application's menus are offered**: they are on this Mac's menu bar, which the
        // session cannot see, so its title bar there gets a menu button that reads them out
        // through this host (`menu(_:)`). A screen is no one application's; one with no menu
        // bar of its own (an accessory) has none to offer.
        if info.display == nil, NSRunningApplication(processIdentifier: info.pid)?.activationPolicy == .regular {
            link.say(["Menu": ["Offered": ["window": Int(id)]]])
        }
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
                    // Not asked for again by itself: a refused capture asked for every tick is a
                    // window that blinks on the session's side. Asked for again, it is tried again.
                    self?.askedFor.remove(info.windowID)
                    self?.captureFailed[info.windowID] = Date()
                }
            }
        }
    }

    private var screenSoundApart: Set<String> = []

    private func listen(to bundle: String) {
        let audio = MacAudio(bundle: bundle, apart: screenSoundApart)
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
        guard !s.paused else { return }
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
            // Its pixels are the Retina screen's: the session sizes it by its points.
            let points = s.capture.pointsPerPixel()
            if points > 0 { link.say(["Density": ["window": Int(id), "pixels_per_point": 1 / points]]) }
        }
        s.encoder?.encode(pixels, microseconds: micros, key: s.wantsKey)
        s.wantsKey = false
    }

    private var perWindowKbit: Int { max(1_000, kbit / max(1, shared.count)) }

    /// A window that is not changing sends nothing; so when a session asks for a picture, encode the last one again.
    /// Not on the main thread, which must never wait for a window's lock: the lock is held while the window's
    /// pictures are encoded, and an encoder that stops answering held it for good (see `HostEncoder.invalidate`).
    private func resend(_ s: Shared) {
        DispatchQueue.global(qos: .userInitiated).async {
            s.lock.lock()
            defer { s.lock.unlock() }
            s.wantsKey = true
            if !s.paused, let pixels = s.lastPixels, let encoder = s.encoder {
                encoder.encode(pixels, microseconds: UInt64(ProcessInfo.processInfo.systemUptime * 1_000_000), key: true)
                s.wantsKey = false
            }
        }
    }

    // MARK: what the session says

    private func windowNumber(_ fields: [String: Any]) -> UInt16? {
        (fields["window"] as? NSNumber).map { UInt16(truncatingIfNeeded: $0.intValue) }
    }

    /// Whether the session attached now keeps Spatiand's applications' settings.
    private var settingsPeer = false
    private var settingsLooked = Date.distantPast

    private func settingsReplies(_ text: UnsafeMutablePointer<CChar>?) -> [Any] {
        guard let text else { return [] }
        defer { sp_settings_free(text) }
        let json = String(cString: text)
        return ((try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [Any]) ?? []
    }

    private func sayAll(_ replies: [Any]) {
        for reply in replies { link?.say(["ConfigSync": reply]) }
    }

    private func said(_ message: [String: Any]) {
        guard let (name, body) = message.first else { return }
        let fields = body as? [String: Any] ?? [:]
        if ProcessInfo.processInfo.environment["SPATIAND_DEBUG"] != nil, name != "InputAt" { print("host: session said \(name) \(fields)") }
        // **A locked Mac is used from nowhere**: no key, pointer, menu, launch, close, resize or
        // bringing forward from the session, only told that it waits (`Held`) -- as no one at
        // its own keyboard could do any of that before unlocking it.
        if macLocked, ["Input", "InputAt", "Menu", "Launch", "Close", "Configure", "Focus", "NoticePress"].contains(name) {
            if name == "Menu" { link?.say(["Menu": "Done"]) }
            if !name.hasPrefix("Input") || !toldLocked { sayHeld() }
            return
        }
        switch name {
        case "ConfigSync":
            // Spatiand's applications' settings: handed to the shared logic, and its answers
            // said back. From now on, what changes here is said too (see `tick`).
            settingsPeer = true
            if let data = try? JSONSerialization.data(withJSONObject: body, options: [.fragmentsAllowed]), let text = String(data: data, encoding: .utf8) {
                sayAll(settingsReplies(sp_settings_said(text)))
            }
        case "Launch":
            if let app = fields["app"] as? String { launch(app) }
        case "Show":
            // (Locked, it is remembered and shown when this Mac is unlocked: the session is
            // waiting for it, and says so.)
            if let id = windowNumber(fields), let info = offered[id] {
                askedFor.insert(info.windowID)
                reconcile()
            }
            if macLocked { sayHeld() }
        case "NoticePress":
            // The card a banner of this Mac's is on was pressed in the session's room: the
            // banner is pressed, which opens what it came from, and that application's windows
            // are shown to the session (`Notices.onOpened`).
            print("host: the session pressed a notification's card")
            // Forgotten if the press opened nothing, and after 2 s whatever happened: a press of
            // the room's own card later is not taken for the session's.
            let press = UUID()
            pressedNotice = press
            Notices.shared.pressed { [weak self] opened in
                if !opened, self?.pressedNotice == press { self?.pressedNotice = nil }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
                if self?.pressedNotice == press { self?.pressedNotice = nil }
            }
        case "Close":
            if let id = windowNumber(fields), let s = shared[id] {
                // A screen is not closed, only no longer shown: and then on offer again.
                if s.info.display != nil { askedFor.remove(s.info.windowID); reconcile() } else { close(s.info) }
            }
        case "Configure":
            if let id = windowNumber(fields), let s = shared[id], let w = (fields["width"] as? NSNumber)?.doubleValue,
               let h = (fields["height"] as? NSNumber)?.doubleValue {
                resize(s, pixelsWide: w, high: h)
            }
        case "Focus":
            focus = windowNumber(fields)
            if let id = focus, let s = shared[id] {
                if onSharedScreen(s.info) {
                    print("host: \(s.info.app) has the session's keys; it is on a screen the session is shown, so the Mac's front window is left as it is until it is pressed")
                } else {
                    NSRunningApplication(processIdentifier: s.info.pid)?.activate()
                }
            }
        case "WantKeyframe":
            if let id = windowNumber(fields), let s = shared[id] { resend(s) }
        case "InputAt", "Input":
            if let id = windowNumber(fields), let s = shared[id], let input = fields["input"] { act(on: s, input) }
        case "Clipboard":
            clipboard.fromHost(fields)
        case "Menu":
            menu(body)
        case "Bandwidth":
            if let k = (fields["max_kbit"] as? NSNumber)?.intValue {
                kbit = min(max(k, 2_000), 40_000)
                // Off the main thread, as `resend` is.
                let each = perWindowKbit
                for s in shared.values {
                    DispatchQueue.global(qos: .utility).async { s.lock.lock(); s.encoder?.setBitrate(each); s.lock.unlock() }
                }
            }
        default:
            break
        }
    }

    // MARK: an application's menus

    /// The menus of the session's windows' applications, read and pressed as in the room
    /// (`RoomMenus`), each level said over the link. On a queue of their own: an application
    /// answers Accessibility when it likes.
    private let menuQueue = DispatchQueue(label: "host-menus")
    private lazy var menus = RoomMenus(say: { [weak self] title, rows in
        self?.link?.say(["Menu": ["Level": ["title": title, "rows": rows.map { ["label": $0.label, "detail": $0.detail] }]]])
    }, done: { [weak self] in
        self?.link?.say(["Menu": "Done"])
    })

    /// What the session said of a window's menus: open them (`{"Open": {"window": n}}`), choose a
    /// row (`{"Choose": {"row": n}}`) or go back up (`"Back"`).
    private func menu(_ body: Any) {
        let menus = self.menus
        if let ask = body as? [String: Any], let (name, value) = ask.first {
            let fields = value as? [String: Any] ?? [:]
            switch name {
            case "Open":
                guard let id = windowNumber(fields), let s = shared[id], s.info.display == nil else {
                    link?.say(["Menu": "Done"])
                    return
                }
                let info = s.info
                print("host: the session opened the menus of \(info.app)")
                // What is chosen is done to this window, not to whichever of the app's is in
                // front: it is brought forward, as the room does with its own (`RoomWindows.menu`).
                NSRunningApplication(processIdentifier: info.pid)?.activate()
                menuQueue.async {
                    if let window = MacWindows.axWindow(info) { AXUIElementPerformAction(window, kAXRaiseAction as CFString) }
                    menus.onChosen = { [weak self] in DispatchQueue.main.async { self?.reconcile() } }
                    menus.open(pid: info.pid, name: info.app)
                }
            case "Choose":
                let row = (fields["row"] as? NSNumber)?.intValue ?? -1
                menuQueue.async { menus.choose(row) }
            default:
                break
            }
        } else if body as? String == "Back" {
            menuQueue.async { menus.back() }
        }
    }

    // MARK: launching, closing, resizing

    private func launch(_ bundle: String) {
        // Nothing named: the session only asked to use this computer.
        guard !bundle.isEmpty else { if macLocked { sayHeld() }; return }
        launched.insert(bundle)
        if macLocked { sayHeld() }
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
            if name == "ScrollStop" { MacInput.real.async { MacInput.pad.stopped() } }
            return
        }
        guard let (kind, body) = (input as? [String: Any])?.first, let fields = body as? [String: Any] else { return }
        switch kind {
        case "Motion":
            guard let x = (fields["x"] as? NSNumber)?.doubleValue, let y = (fields["y"] as? NSNumber)?.doubleValue else { return }
            let type: CGEventType = pressedDown ? (pressedButton == .right ? .rightMouseDragged : .leftMouseDragged) : .mouseMoved
            let point = s.capture.screenPoint(x: x, y: y)
            let (down, button) = (pressedDown, pressedButton)
            // The session says where the pointer is every frame, moved or not. Said again it is
            // nothing: on a screen it would pull the Mac's own pointer back, over and over,
            // from whoever is moving it there.
            let same = lastPoint == (x, y)
            lastPoint = (x, y)
            if same { return }
            // Carried with the button down, it is the real pointer that is carried; only
            // pointed, the application is told and the real pointer is left where it is.
            MacInput.moved {
                // In a screen the pointer is the Mac's real one, pressed or not.
                if down || info.display != nil { MacInput.realMouse(type, button: button, at: point, clicks: 1) }
                else { MacInput.mouse(type, button: button, at: point, clicks: 1, window: info.windowID, pid: info.pid) }
            }
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
            let others = Set(shared.values.map(\.info.windowID)).union(offered.values.map(\.windowID))
            let move = MacInput.takeMove()
            MacInput.real.async {
                move?()
                if down, info.display == nil { MacInput.bringForward(info, at: point, others: others) }
                MacInput.realMouse(type, button: button, at: point, clicks: clicks)
            }
        case "Scroll":
            let h = (fields["horizontal"] as? NSNumber)?.doubleValue ?? 0
            let v = (fields["vertical"] as? NSNumber)?.doubleValue ?? 0
            // A notch is a few units and a trackpad's pixels are many; a notch is about forty pixels.
            func pixels(_ d: Double) -> Int32 { Int32(-d * (abs(d) <= 3 ? 40 : 1)) }
            let point = s.capture.screenPoint(x: lastPoint.0, y: lastPoint.1)
            let others = Set(shared.values.map(\.info.windowID)).union(offered.values.map(\.windowID))
            let again = Date().timeIntervalSince(scrolledAt) > 0.5
            scrolledAt = Date()
            MacInput.real.async {
                // Scrolled where the real pointer is, so the window has to be what is under it.
                if again, info.display == nil { MacInput.bringForward(info, at: point, others: others) }
                MacInput.realScroll(dx: pixels(h), dy: pixels(v), at: point)
            }
        case "FingerScroll":
            // A pad's, exact. Taken as points, not the picture's pixels: the room's swipe is
            // measured for a window's points (it says the window's density, `Density`), and in
            // pixels a Retina window scrolled half as far as any other for the same swipe.
            let h = -((fields["horizontal"] as? NSNumber)?.doubleValue ?? 0)
            let v = -((fields["vertical"] as? NSNumber)?.doubleValue ?? 0)
            let point = s.capture.screenPoint(x: lastPoint.0, y: lastPoint.1)
            let others = Set(shared.values.map(\.info.windowID)).union(offered.values.map(\.windowID))
            let again = Date().timeIntervalSince(scrolledAt) > 0.5
            scrolledAt = Date()
            MacInput.pad.heard(dx: h, dy: v)
            MacInput.real.async {
                if again, info.display == nil { MacInput.bringForward(info, at: point, others: others) }
                MacInput.pad.moved(at: point, window: info.windowID)
            }
        case "Key":
            guard let code = (fields["code"] as? NSNumber)?.uint32Value, let down = fields["pressed"] as? Bool else { return }
            if down { held.insert(code) } else { held.remove(code) }
            // The modifiers ride on the keys that follow; they are not keys of their own here.
            if [42, 54, 29, 97, 56, 100, 125, 126, 58].contains(code) { return }
            // (As macOS calls the key on this Mac's keyboard: an ISO one has two the other way about.)
            guard let mac = Self.macKey[code].map(KeyMap.place) else { return }
            // A screen's keys go to whatever has the keys on the Mac, as a keyboard's do.
            MacInput.key(code: mac, flags: NSEvent.ModifierFlags(rawValue: UInt(flags(for: info.bundle).rawValue)), down: down, pid: info.display != nil ? nil : info.pid)
        default:
            break
        }
    }

    private var pressedDown = false
    private var scrolledAt = Date.distantPast
    private var lastPoint: (Double, Double) = (0, 0)
}
