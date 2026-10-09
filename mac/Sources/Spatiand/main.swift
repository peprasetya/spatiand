//  main.swift — Spatiand's menu-bar app.
//
//  The whole interface until something is open is one icon at the top right of the screen. Its
//  menu says what Spatiand is doing, where its windows go, and where its sound plays. Everything
//  else -- the computers it is paired with, the windows it is showing -- will appear in it as the
//  parts that make them are written; today those sections say so.
//
//  `Spatiand --selftest` prints what it can see and exits, for checking without a screen.

import AppKit
import CSpatiand

setvbuf(stdout, nil, _IOLBF, 0)

// The measured head the app carries, if it does: told to the audio code by the environment, which is
// all it reads. A copy run from the build folder has none and uses the parametric head.
if let frameworks = Bundle.main.privateFrameworksPath, let resources = Bundle.main.resourcePath,
   FileManager.default.fileExists(atPath: frameworks + "/libmysofa.1.dylib"),
   FileManager.default.fileExists(atPath: resources + "/default.sofa") {
    setenv("SPATIAND_MYSOFA_LIBRARY", frameworks + "/libmysofa.1.dylib", 0)
    setenv("SPATIAND_HRTF_DATASET", resources + "/default.sofa", 0)
}

// Run from its bundle, Spatiand has no terminal, so what it says goes to a log, kept to a size that
// does not matter: ~/Library/Logs/Spatiand/spatiand.log. Run from a shell it still says it there.
if Bundle.main.bundleURL.pathExtension == "app", isatty(STDOUT_FILENO) == 0 {
    let dir = NSString("~/Library/Logs/Spatiand").expandingTildeInPath
    try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    let path = dir + "/spatiand.log"
    if let size = (try? FileManager.default.attributesOfItem(atPath: path))?[.size] as? Int, size > 2_000_000 {
        try? FileManager.default.removeItem(atPath: path + ".old")
        try? FileManager.default.moveItem(atPath: path, toPath: path + ".old")
    }
    var info = stat()
    // Only when stdout is nowhere useful: `open --stdout` or a shell redirect is left alone.
    if fstat(STDOUT_FILENO, &info) == 0, (info.st_mode & S_IFMT) != S_IFREG {
        freopen(path, "a", stdout)
        freopen(path, "a", stderr)
        setvbuf(stdout, nil, _IOLBF, 0)
        print("--- Spatiand started \(Date())")
    }
}

final class App: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let glasses = GlassesWatcher()
    private let pairing = PairingUI()
    private let hotkeys = Hotkeys()
    private lazy var settings = SettingsWindow(hotkeyStatus: { [unowned self] in
        let menu = hotkeys.status[.menu] == true
        let tab = hotkeys.status[.settings] == true
        if menu && tab { return "Both chords are active." }
        var problems: [String] = []
        if !menu { problems.append("Ctrl-Space is taken by macOS (Keyboard \u{2192} Keyboard Shortcuts \u{2192} Input Sources)") }
        if !tab { problems.append("Ctrl-Tab could not be registered") }
        return problems.joined(separator: "; ") + "."
    })

    private var signalSources: [DispatchSourceSignal] = []

    /// The glasses go back to their ordinary mode and the Mac gets its mouse back, however the app
    /// is ended: from the menu, by `kill`, or with Ctrl-C.
    func applicationWillTerminate(_ note: Notification) { giveEverythingBack() }

    private func giveEverythingBack() {
        Room.shared.stop()
    }

    private func handleSignals() {
        for number in [SIGTERM, SIGINT, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { [weak self] in
                self?.giveEverythingBack()
                // A moment for the glasses to hear it before the cable's other end goes quiet.
                RunLoop.main.run(until: Date().addingTimeInterval(0.3))
                exit(0)
            }
            source.resume()
            signalSources.append(source)
        }
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        // One Spatiand at a time: a second would fight the first for the glasses and the host.
        guard OnlyOne.take() else {
            print("Spatiand is already running; this one stops")
            exit(0)
        }
        handleSignals()
        if let button = item.button {
            button.image = NSImage(systemSymbolName: "eyeglasses", accessibilityDescription: "Spatiand")
        }
        let menu = NSMenu()
        menu.delegate = self
        item.menu = menu
        glasses.onChange = { [weak self] _ in self?.refreshIcon() }
        glasses.start()
        refreshIcon()
        if Settings.hostEnabled { MacHost.shared.start() }
        wasPlugged = glasses.isPluggedIn
        pairing.start()
        // With the glasses on and the mouse and keyboard theirs, these are the room's own launcher
        // and settings: the Mac's menu is out of sight. Otherwise they are the Mac's.
        hotkeys.onMenu = { [weak self] in
            if Room.shared.running, RoomTap.shared.holding { Self.press(1) } else { self?.item.button?.performClick(nil) }
        }
        hotkeys.onSettings = { [weak self] in
            if Room.shared.running, RoomTap.shared.holding { Self.press(0) } else { self?.settings.show() }
        }
        if Settings.hotkeys { hotkeys.enable() }
        Model.shared.onProblem = { PairingUI.alert("Spatiand", $0) }
        Room.shared.onChange = { [weak self] in self?.refreshIcon() }
        // The display goes away for a moment whenever the glasses change mode -- which they do as
        // the room starts, going to two eyes -- and comes back: wait for it again. Unplugged for
        // good is the USB going, which `refreshIcon` hears.
        Room.shared.screen.onLost = { Room.shared.screen.start() }
        chooseWorld()
    }

    /// One of the Deck's buttons, pressed and let go: 0 the settings, 1 the launcher.
    private static func press(_ control: Int32) {
        sp_control(control, true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { sp_control(control, false) }
    }

    /// With glasses, the room: the Deck's compositor, in the glasses. Without, windows on this
    /// Mac. Never both: a host shows its windows to one viewer, and the room is its own.
    private func chooseWorld() {
        Model.shared.glassesOn = false
        if glassesWanted() {
            guard !Room.shared.running else { return }
            Model.shared.disconnect()
            Room.shared.start()
        } else if Room.shared.running {
            // One session a process; see `Room`.
            relaunchForNextPlugIn()
        } else if Model.shared.host == nil, let last = Hosts.all.first {
            // Back to the computer this Mac was last using, if there is one.
            Model.shared.connect(last)
        }
    }

    /// The icon says which world Spatiand is in: dimmed with nothing plugged in, normal with
    /// glasses.
    private func refreshIcon() {
        item.button?.appearsDisabled = !glasses.isPluggedIn
        let unplugged = wasPlugged && !glasses.isPluggedIn
        wasPlugged = glasses.isPluggedIn
        // A display that has gone leaves a ghost of the glasses' window in the window server for
        // as long as this process lives, so the next plug-in starts from a fresh one.
        if unplugged, Room.shared.running { relaunchForNextPlugIn(); return }
        if glassesWanted() != Room.shared.running { chooseWorld() }
    }

    private var wasPlugged = false
    /// This Mac's windows, as of the last time the menu opened.
    private var macList: [MacWindowInfo] = []

    /// Start over as a new process, once this one has gone. Through `open`, so macOS treats the
    /// new one as the app and not as a child of whatever started this.
    private func relaunchForNextPlugIn() {
        // Once: stopping the room says things have changed, which must not come back here.
        guard !leaving else { return }
        leaving = true
        Room.shared.onChange = nil
        glasses.onChange = nil
        let bundle = Bundle.main.bundleURL
        guard bundle.pathExtension == "app" else {
            print("the room has ended; not running from an app, so stopping here")
            Room.shared.stop()
            exit(0)
        }
        let relauncher = Process()
        relauncher.executableURL = URL(fileURLWithPath: "/bin/sh")
        // `open` can refuse in the moment after the old one goes (-600); it is asked until it does not.
        relauncher.arguments = ["-c", "while kill -0 \(getpid()) 2>/dev/null; do sleep 0.2; done; for i in 1 2 3 4 5 6 7 8; do /usr/bin/open -n \"$1\" && exit 0; sleep 0.5; done", "sh", bundle.path]
        do { try relauncher.run() } catch { print("could not relaunch: \(error)") }
        print("the room has ended; restarting so the next one starts clean")
        Room.shared.stop()
        exit(0)
    }

    private var leaving = false

    /// Glasses plugged in, and the owner has not said to keep everything on the Mac.
    private func glassesWanted() -> Bool {
        glasses.isPluggedIn && Settings.presentation == .automatic
    }

    // MARK: the menu, rebuilt each time it opens so it is never out of date

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()

        let room = Room.shared
        let status = glasses.isPluggedIn
            ? (room.running ? "Glasses: " + room.status : "Glasses connected \u{2014} windows are on this Mac")
            : "No glasses \u{2014} windows are on this Mac"
        add(menu, status, enabled: false)
        if room.running {
            if !RoomTap.allowed(ask: false) {
                let ask = NSMenuItem(title: "Allow Accessibility, for the mouse and keyboard in the glasses\u{2026}", action: #selector(allowControl), keyEquivalent: "")
                ask.target = self
                menu.addItem(ask)
            }
            let hold = NSMenuItem(title: RoomTap.shared.holding ? "Give the mouse and keyboard back to this Mac (Ctrl-Option-G)" : "Use this Mac\u{2019}s mouse and keyboard in the glasses (Ctrl-Option-G)",
                                  action: #selector(toggleCapture), keyEquivalent: "")
            hold.target = self
            menu.addItem(hold)
            let centre = NSMenuItem(title: "Recentre the view (Ctrl-Option-R)", action: #selector(recentre), keyEquivalent: "")
            centre.target = self
            menu.addItem(centre)
        }
        menu.addItem(.separator())

        if room.running {
            let macs = NSMenuItem(title: "Bring a window of this Mac into the glasses", action: nil, keyEquivalent: "")
            let sub = NSMenu()
            if !MacWindows.allowed(ask: false) {
                let ask = NSMenuItem(title: "Allow Screen Recording\u{2026}", action: #selector(allowCapture), keyEquivalent: "")
                ask.target = self
                sub.addItem(ask)
            } else if macList.isEmpty {
                let none = NSMenuItem(title: "No windows to show", action: nil, keyEquivalent: "")
                none.isEnabled = false
                sub.addItem(none)
            }
            for (i, info) in macList.prefix(30).enumerated() {
                let entry = NSMenuItem(title: info.app + (info.title.isEmpty ? "" : " \u{2014} " + String(info.title.prefix(40))),
                                       action: #selector(bringMacWindow(_:)), keyEquivalent: "")
                entry.target = self
                entry.tag = i
                entry.state = RoomWindows.shared.has(info.windowID) ? .on : .off
                sub.addItem(entry)
            }
            macs.submenu = sub
            menu.addItem(macs)
            // For the next time the menu opens: asking takes a moment, and a menu cannot wait.
            Task { @MainActor in self.macList = await MacWindows.list() }
        } else {
            buildComputers(menu)
        }
        menu.addItem(.separator())

        let present = NSMenuItem(title: "Show windows", action: nil, keyEquivalent: "")
        let presentMenu = NSMenu()
        for choice in Presentation.allCases {
            let entry = NSMenuItem(title: choice.title, action: #selector(choosePresentation(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = choice.rawValue
            entry.state = Settings.presentation == choice ? .on : .off
            presentMenu.addItem(entry)
        }
        present.submenu = presentMenu
        menu.addItem(present)

        buildHost(menu)

        let audio = NSMenuItem(title: "Sound output", action: nil, keyEquivalent: "")
        let audioMenu = NSMenu()
        let follow = NSMenuItem(title: "Same as this Mac", action: #selector(chooseOutput(_:)), keyEquivalent: "")
        follow.target = self
        follow.state = Settings.audioOutputUID == nil ? .on : .off
        audioMenu.addItem(follow)
        audioMenu.addItem(.separator())
        for device in AudioDevices.outputs() {
            let entry = NSMenuItem(title: device.name, action: #selector(chooseOutput(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = device.uid
            entry.state = Settings.audioOutputUID == device.uid ? .on : .off
            audioMenu.addItem(entry)
        }
        audio.submenu = audioMenu
        menu.addItem(audio)

        menu.addItem(.separator())
        let preferences = NSMenuItem(title: "Settings\u{2026}", action: #selector(openSettings), keyEquivalent: ",")
        preferences.target = self
        menu.addItem(preferences)
        let quit = NSMenuItem(title: "Quit Spatiand", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        menu.addItem(quit)
    }

    /// This Mac as a computer other devices can use the windows of.
    private func buildHost(_ menu: NSMenu) {
        let host = MacHost.shared
        let top = NSMenuItem(title: "Let other devices use this Mac\u{2019}s windows", action: #selector(toggleHost), keyEquivalent: "")
        top.target = self
        top.state = host.running ? .on : .off
        menu.addItem(top)
        guard host.running else { return }
        add(menu, "   \(host.status) \u{2014} \(host.address)", enabled: false)
        if !MacWindows.allowed(ask: false) {
            let ask = NSMenuItem(title: "   Allow Screen Recording\u{2026}", action: #selector(allowCapture), keyEquivalent: "")
            ask.target = self
            menu.addItem(ask)
        }
        if !MacInput.allowed(ask: false) {
            let ask = NSMenuItem(title: "   Allow Accessibility, to be clicked and typed into\u{2026}", action: #selector(allowControl), keyEquivalent: "")
            ask.target = self
            menu.addItem(ask)
        }
        let pairing = NSMenuItem(title: host.pairing ? "   Stop letting a new device pair" : "   Let a new device pair (two minutes)",
                                 action: #selector(togglePairing), keyEquivalent: "")
        pairing.target = self
        menu.addItem(pairing)
        if host.pairedCount > 0 {
            let forget = NSMenuItem(title: "   Forget the \(host.pairedCount) paired device\(host.pairedCount == 1 ? "" : "s")", action: #selector(forgetDevices), keyEquivalent: "")
            forget.target = self
            menu.addItem(forget)
        }
    }

    @objc private func toggleHost() {
        Settings.hostEnabled.toggle()
        if Settings.hostEnabled { MacHost.shared.start() } else { MacHost.shared.stop() }
    }
    @objc private func togglePairing() {
        if MacHost.shared.pairing { MacHost.shared.closePairing() } else { MacHost.shared.openPairing() }
    }
    @objc private func forgetDevices() { MacHost.shared.forgetAllDevices() }
    @objc private func allowControl() { _ = MacInput.allowed(ask: true) }

    private func buildComputers(_ menu: NSMenu) {
        let model = Model.shared
        add(menu, "Computers", enabled: false)
        if Hosts.all.isEmpty { add(menu, "   None paired yet", enabled: false) }
        for host in Hosts.all {
            let isCurrent = model.host == host
            let state = isCurrent ? (model.connected ? "  \u{2014} connected" : model.takenOver ? "  \u{2014} in use on another device" : "  \u{2014} connecting\u{2026}") : ""
            let entry = NSMenuItem(title: "   " + host.name + state, action: nil, keyEquivalent: "")
            let sub = NSMenu()
            if isCurrent, model.takenOver, !model.connected {
                let join = NSMenuItem(title: "Take the windows back", action: #selector(connectHost(_:)), keyEquivalent: "")
                join.target = self
                join.representedObject = host.fingerprint
                sub.addItem(join)
            } else if isCurrent {
                if model.connected {
                    for app in model.apps {
                        let open = NSMenuItem(title: "Open " + app.name, action: #selector(openApp(_:)), keyEquivalent: "")
                        open.target = self
                        open.representedObject = app.id
                        sub.addItem(open)
                    }
                    sub.addItem(.separator())
                }
                let leave = NSMenuItem(title: "Disconnect", action: #selector(disconnect), keyEquivalent: "")
                leave.target = self
                sub.addItem(leave)
            } else {
                let join = NSMenuItem(title: "Connect", action: #selector(connectHost(_:)), keyEquivalent: "")
                join.target = self
                join.representedObject = host.fingerprint
                sub.addItem(join)
            }
            sub.addItem(.separator())
            let forget = NSMenuItem(title: "Forget this computer", action: #selector(forgetHost(_:)), keyEquivalent: "")
            forget.target = self
            forget.representedObject = host.fingerprint
            sub.addItem(forget)
            entry.submenu = sub
            menu.addItem(entry)
        }
        let add = NSMenuItem(title: "   Add a computer\u{2026}", action: #selector(addComputer), keyEquivalent: "")
        add.target = self
        menu.addItem(add)

        menu.addItem(.separator())
        self.add(menu, "Windows", enabled: false)
        let open = model.openWindows
        if open.isEmpty { self.add(menu, "   Nothing open", enabled: false) }
        for window in open {
            let entry = NSMenuItem(title: "   " + window.title, action: #selector(raiseWindow(_:)), keyEquivalent: "")
            entry.target = self
            entry.tag = Int(window.id)
            menu.addItem(entry)
        }
    }

    @objc private func allowCapture() { _ = MacWindows.allowed(ask: true) }
    @objc private func bringMacWindow(_ sender: NSMenuItem) {
        guard sender.tag < macList.count else { return }
        RoomWindows.shared.bring(macList[sender.tag])
    }

    @objc private func toggleCapture() {
        if !RoomTap.shared.holding, !RoomTap.allowed(ask: true) { return }
        RoomTap.shared.toggle()
    }
    @objc private func recentre() { sp_recentre() }
    @objc private func openSettings() { settings.show() }
    @objc private func addComputer() { pairing.ask() }
    @objc private func disconnect() { Model.shared.disconnect() }
    @objc private func openApp(_ sender: NSMenuItem) {
        if let id = sender.representedObject as? String, let app = Model.shared.apps.first(where: { $0.id == id }) {
            Model.shared.launch(app)
        }
    }
    @objc private func connectHost(_ sender: NSMenuItem) {
        if let fp = sender.representedObject as? String, let host = Hosts.all.first(where: { $0.fingerprint == fp }) {
            Model.shared.connect(host)
        }
    }
    @objc private func forgetHost(_ sender: NSMenuItem) {
        if let fp = sender.representedObject as? String, let host = Hosts.all.first(where: { $0.fingerprint == fp }) {
            Model.shared.forget(host)
        }
    }
    @objc private func raiseWindow(_ sender: NSMenuItem) { Model.shared.raise(UInt16(sender.tag)) }

    private func add(_ menu: NSMenu, _ title: String, enabled: Bool) {
        let entry = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        entry.isEnabled = enabled
        menu.addItem(entry)
    }

    @objc private func choosePresentation(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let choice = Presentation(rawValue: raw) {
            Settings.presentation = choice
            chooseWorld()
        }
    }

    @objc private func chooseOutput(_ sender: NSMenuItem) {
        Settings.audioOutputUID = sender.representedObject as? String
        AudioOut.shared.applyDevice()
        let device = AudioDevices.outputs().first { $0.uid == Settings.audioOutputUID }
        sp_audio_output(Int32(bitPattern: device?.id ?? 0))
    }
}

if CommandLine.arguments.contains("--selftest") {
    print("outputs:")
    let current = AudioDevices.systemDefault()
    for d in AudioDevices.outputs() {
        print("  \(d.name)\(d == current ? "  (this Mac's)" : "")")
    }
    let watcher = GlassesWatcher()
    watcher.start()
    // Matching callbacks for devices already present arrive as the run loop turns.
    RunLoop.main.run(until: Date().addingTimeInterval(1.0))
    print("glasses plugged in: \(watcher.isPluggedIn) (\(watcher.count))")
    print("show windows: \(Settings.presentation.title)")
    exit(0)
}

if let at = CommandLine.arguments.firstIndex(of: "--selftest-session") {
    // --selftest-session <address> <fingerprint> <app> [seconds]
    let args = Array(CommandLine.arguments[(at + 1)...])
    guard args.count >= 3 else { print("usage: --selftest-session <address> <fingerprint> <app> [seconds]"); exit(2) }
    let dir = NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
    let test = SessionTest(identityDir: dir, output: "/tmp/spatiand-selftest.png")
    exit(test.run(address: args[0], fingerprint: args[1], app: args[2], seconds: Double(args.count > 3 ? args[3] : "") ?? 8))
}

if let at = CommandLine.arguments.firstIndex(of: "--add-host") {
    // --add-host <name> <address> <fingerprint>: a computer this Mac is already paired with.
    let args = Array(CommandLine.arguments[(at + 1)...])
    guard args.count >= 3 else { print("usage: --add-host <name> <address> <fingerprint>"); exit(2) }
    Hosts.add(PairedHost(name: args[0], address: args[1], fingerprint: args[2]))
    print("known computers: \(Hosts.all.map(\.name).joined(separator: ", "))")
    exit(0)
}


if let at = CommandLine.arguments.firstIndex(of: "--preview") {
    // --preview [seconds] [application.app]: the room in a window on this Mac, with stand-in
    // glasses, for looking at without wearing anything. With an application, its windows are
    // brought in. A picture of it is saved every few seconds (~/screenshots).
    let args = Array(CommandLine.arguments[(at + 1)...])
    let seconds = Double(args.first ?? "") ?? 20
    setenv("SPATIAND_HMD", "null", 1)
    // Its own preferences, so that looking at it changes nothing of the owner's.
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        // The stand-in glasses do not know which way up they are, and ask; B says never mind.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            sp_control(3, true)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { sp_control(3, false) }
        }
        if args.count > 1 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { Room.shared.launch(path: args[1]) }
        }
        let shots = Timer(timeInterval: 4, repeats: true) { _ in sp_screenshot() }
        RunLoop.main.add(shots, forMode: .common)
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            Room.shared.stop()
            exit(0)
        }
    }
    application.run()
}

if CommandLine.arguments.contains("--selftest-notice-room") {
    // The room in a window, a notification posted, and a picture of the room with it on show.
    setenv("SPATIAND_HMD", "null", 1)
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        after(1.5) { sp_control(3, true); after(0.1) { sp_control(3, false) } }
        after(3) {
            let script = Process()
            script.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
            script.arguments = ["-e", "display notification \"Lunch is at one, at the usual place.\" with title \"Spatiand test\""]
            try? script.run()
        }
        after(6) { sp_screenshot() }
        after(9) { Room.shared.stop(); exit(0) }
    }
    application.run()
}

if CommandLine.arguments.contains("--selftest-notice") {
    // Post a notification of our own and say what is read of the banner it makes.
    let script = Process()
    script.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    script.arguments = ["-e", "display notification \"The words of the test.\" with title \"Spatiand test\" subtitle \"A subtitle\""]
    try? script.run()
    print("accessibility allowed: \(AXIsProcessTrusted())")
    for attempt in 0..<12 {
        usleep(500_000)
        let found = Notices.banners()
        print("look \(attempt): \(found.count) banner(s) \(found.map { $0.1 })")
        if !found.isEmpty { break }
    }
    exit(0)
}

if CommandLine.arguments.contains("--selftest-windows") {
    // The room in a window, and its list of windows opened (the settings, then Windows): every
    // window open on this Mac should be in it, put away. A picture is saved (~/screenshots).
    setenv("SPATIAND_HMD", "null", 1)
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
    func press(_ control: Int32, at time: Double) { after(time) { sp_control(control, true); after(0.08) { sp_control(control, false) } } }
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        press(3, at: 1.5)
        press(0, at: 4.0)
        for step in 0..<6 { press(7, at: 4.5 + Double(step) * 0.3) }
        press(2, at: 6.6)
        after(7.5) { sp_screenshot() }
        // With --and-choose, the first window in the list is brought out: its picture should be
        // in the second screenshot.
        if CommandLine.arguments.contains("--and-choose") {
            press(2, at: 9.0)
            after(13) { sp_screenshot() }
            after(16) { Room.shared.stop(); exit(0) }
        } else {
            after(10) { Room.shared.stop(); exit(0) }
        }
    }
    application.run()
}

if let at = CommandLine.arguments.firstIndex(of: "--selftest-click") {
    // --selftest-click [application.app]: the room in a window, an application's window brought
    // in, and the room's pointer -- which starts in the middle of the view, where the window is --
    // clicked twice. Pictures before and after are saved (~/screenshots). Moves the Mac's real
    // pointer and brings the application to the front: not for while the Mac is in use.
    let path = CommandLine.arguments.dropFirst(at + 1).first ?? "/System/Applications/Calculator.app"
    setenv("SPATIAND_HMD", "null", 1)
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        after(1.5) { sp_control(3, true); after(0.1) { sp_control(3, false) } }
        after(2) { Room.shared.launch(path: path) }
        after(6) { sp_pointer(-17, 150, 0, 0, 0); sp_screenshot() }
        for press in [7.5, 8.5] {
            after(press) { sp_pointer(0, 0, 1, 0, 0); after(0.1) { sp_pointer(0, 0, 0, 0, 0) } }
        }
        after(10) { sp_screenshot() }
        after(13) { Room.shared.stop(); exit(0) }
    }
    application.run()
}

if CommandLine.arguments.contains("--selftest-resize") {
    // The room in a window, the Dictionary brought in, and its window asked to be a new size
    // sixty times a second for two seconds, as a drag by its corner does. Says how long the real
    // window took to arrive at the last size asked for, and the longest the main thread -- where
    // the mouse comes in -- was kept waiting meanwhile.
    setenv("SPATIAND_HMD", "null", 1)
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
    var worstStall = 0.0
    var lastBeat = Date()
    let beat = Timer(timeInterval: 0.01, repeats: true) { _ in
        let now = Date()
        worstStall = max(worstStall, now.timeIntervalSince(lastBeat))
        lastBeat = now
    }
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        after(1.5) { sp_control(3, true); after(0.1) { sp_control(3, false) } }
        after(2) { Room.shared.launch(path: "/System/Applications/Dictionary.app") }
        after(8) {
            guard let window = RoomWindows.shared.shownWindow(bundle: "com.apple.Dictionary") else {
                print("resize: FAILED, the Dictionary's window never came into the room")
                Room.shared.stop(); exit(1)
            }
            let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
            let before = MacWindows.currentFrame(window.info)?.size ?? .zero
            print("resize: the window is \(Int(before.width))x\(Int(before.height)) points")
            RunLoop.main.add(beat, forMode: .common)
            lastBeat = Date()
            worstStall = 0
            let started = Date()
            let last = CGSize(width: before.width + 120, height: before.height + 80)
            for step in 1...120 {
                after(Double(step) / 60) {
                    let w = Double(before.width) + Double(step), h = Double(before.height) + Double(step) * 2 / 3
                    RoomWindows.shared.asked(Int32(SP_WINDOW_RESIZE), id: window.id, a: w * scale, b: h * scale, c: 0)
                }
            }
            func settled(_ tries: Int) {
                let now = MacWindows.currentFrame(window.info)?.size ?? .zero
                let took = Date().timeIntervalSince(started)
                if abs(now.width - last.width) <= 2, abs(now.height - last.height) <= 2 {
                    print(String(format: "resize: PASSED, at %dx%d %.2f s after the drag began (the drag took 2.00); the main thread waited at most %.0f ms", Int(now.width), Int(now.height), took, worstStall * 1000))
                } else if tries > 0 {
                    after(0.1) { settled(tries - 1) }
                    return
                } else {
                    print(String(format: "resize: FAILED, still %dx%d after %.1f s, wanted %dx%d; the main thread waited at most %.0f ms", Int(now.width), Int(now.height), took, Int(last.width), Int(last.height), worstStall * 1000))
                }
                // And smaller again, which is where a black band was left.
                MacWindows.resize(window.info, toPoints: CGSize(width: before.width - 60, height: before.height - 120))
                after(3) {
                    NSRunningApplication(processIdentifier: window.info.pid)?.terminate()
                    after(1) { Room.shared.stop(); exit(0) }
                }
            }
            after(2.05) { settled(60) }
        }
    }
    application.run()
}

if let at = CommandLine.arguments.firstIndex(of: "--click-target") {
    PointerTest.target(file: CommandLine.arguments.dropFirst(at + 1).first ?? NSTemporaryDirectory() + "spatiand-clicks.txt")
}
if CommandLine.arguments.contains("--selftest-pointer") { PointerTest.run() }
if CommandLine.arguments.contains("--selftest-record") {
    // The room in a window, filmed for four seconds: says where the film is and how big.
    setenv("SPATIAND_HMD", "null", 1)
    setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async {
        Room.shared.start(preview: true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { if #available(macOS 15.0, *) { RoomRecorder.shared.start(screen: Room.shared.screen) } else { print("recording: needs macOS 15") } }
        DispatchQueue.main.asyncAfter(deadline: .now() + 8) { if #available(macOS 15.0, *) { RoomRecorder.shared.stop() } else { print("recording: needs macOS 15") } }
        DispatchQueue.main.asyncAfter(deadline: .now() + 11) { Room.shared.stop(); exit(0) }
    }
    application.run()
}


if CommandLine.arguments.contains("--selftest-local") {
    exit(LocalTests.run())
}

if let at = CommandLine.arguments.firstIndex(of: "--serve-host") {
    // --serve-host <seconds>: this Mac as a host and nothing else, for another program to connect to.
    let seconds = Double(CommandLine.arguments.dropFirst(at + 1).first ?? "") ?? 60
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async {
        MacHost.shared.start()
        MacHost.shared.openPairing()
        print("serving as \(MacHost.shared.address), fingerprint \(MacHost.shared.link?.fingerprint ?? "?")")
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
    }
    application.run()
}

if let at = CommandLine.arguments.firstIndex(of: "--selftest-host") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async { HostTest.run(bundle: args.first ?? "com.apple.Terminal") }
    application.run()
}



let application = NSApplication.shared
let delegate = App()
application.delegate = delegate
// A menu-bar app: no Dock icon and no main window.
application.setActivationPolicy(.accessory)
application.run()

/// A lock only one running Spatiand holds, let go of by the system when the process ends.
enum OnlyOne {
    private static var held: Int32 = -1

    /// Whether this process is the one. Waits a moment, for a predecessor that is on its way out.
    static func take() -> Bool {
        let path = NSTemporaryDirectory() + "spatiand.lock"
        let fd = open(path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { return true }
        for _ in 0..<20 {
            if flock(fd, LOCK_EX | LOCK_NB) == 0 { held = fd; return true }
            usleep(100_000)
        }
        close(fd)
        return false
    }
}
