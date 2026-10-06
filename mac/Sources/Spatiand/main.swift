//  main.swift — Spatiand's menu-bar app.
//
//  The whole interface until something is open is one icon at the top right of the screen. Its
//  menu says what Spatiand is doing, where its windows go, and where its sound plays. Everything
//  else -- the computers it is paired with, the windows it is showing -- will appear in it as the
//  parts that make them are written; today those sections say so.
//
//  `Spatiand --selftest` prints what it can see and exits, for checking without a screen.

import AppKit

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
        let room = Model.shared.room
        room.input.stop()
        room.output.stop(restoreMode: true)
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
        Model.shared.room.drivesGlasses = true
        PadInput.shared.start()
        Model.shared.room.onChange = { [weak self] in self?.item.button?.appearsDisabled = !(self?.glasses.isPluggedIn ?? false) }
        wasPlugged = glasses.isPluggedIn
        Model.shared.glassesOn = glassesWanted()
        pairing.start()
        NotificationCenter.default.addObserver(forName: .spatiandPair, object: nil, queue: .main) { [weak self] note in
            if let address = note.object as? String { self?.pairing.begin(address: address) }
        }
        NotificationCenter.default.addObserver(forName: .spatiandLeave, object: nil, queue: .main) { [weak self] _ in
            Settings.presentation = .onThisMac
            Model.shared.glassesOn = self?.glassesWanted() ?? false
            Model.shared.room.setActive(false)
        }
        // With the glasses on and the mouse and keyboard theirs, the menu is the glasses' own: the
        // Mac's is out of sight. Otherwise it is the usual one.
        hotkeys.onMenu = { [weak self] in
            let room = Model.shared.room
            if room.active, room.input.capturing { room.toggleMenu() } else { self?.item.button?.performClick(nil) }
        }
        hotkeys.onSettings = { [weak self] in
            let room = Model.shared.room
            if room.active, room.input.capturing { room.toggleSettings() } else { self?.settings.show() }
        }
        if Settings.hotkeys { hotkeys.enable() }
        Model.shared.onProblem = { PairingUI.alert("Spatiand", $0) }
        // Back to the computer this Mac was last using, if there is one.
        if let last = Hosts.all.first { Model.shared.connect(last) }
    }

    /// The icon says which world Spatiand is in: dimmed with nothing plugged in, normal with
    /// glasses.
    private func refreshIcon() {
        item.button?.appearsDisabled = !glasses.isPluggedIn
        let unplugged = wasPlugged && !glasses.isPluggedIn
        wasPlugged = glasses.isPluggedIn
        Model.shared.glassesOn = glassesWanted()
        // A display that has gone leaves a ghost of the glasses' window in the window server for
        // as long as this process lives, so the next plug-in starts from a fresh one.
        if unplugged, Model.shared.room.output.hasHadAWindow { relaunchForNextPlugIn() }
    }

    private var wasPlugged = false

    /// Start over as a new process, once this one has gone. Through `open`, so macOS treats the
    /// new one as the app and not as a child of whatever started this.
    private func relaunchForNextPlugIn() {
        let bundle = Bundle.main.bundleURL
        guard bundle.pathExtension == "app" else { print("glasses unplugged; not running from an app, staying up"); return }
        let relauncher = Process()
        relauncher.executableURL = URL(fileURLWithPath: "/bin/sh")
        relauncher.arguments = ["-c", "while kill -0 \(getpid()) 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$1\"", "sh", bundle.path]
        do { try relauncher.run() } catch { print("could not relaunch: \(error)"); return }
        print("glasses unplugged; restarting so the next plug-in starts clean")
        Model.shared.room.output.stop(restoreMode: false)
        exit(0)
    }

    /// Glasses plugged in, and the owner has not said to keep everything on the Mac.
    private func glassesWanted() -> Bool {
        glasses.isPluggedIn && Settings.presentation == .automatic
    }

    // MARK: the menu, rebuilt each time it opens so it is never out of date

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()

        let room = Model.shared.room
        let status = glasses.isPluggedIn
            ? (room.active ? "Glasses: " + room.output.status : "Glasses connected — windows are on this Mac")
            : "No glasses — windows are on this Mac"
        add(menu, status, enabled: false)
        if room.active {
            let hold = NSMenuItem(title: room.input.capturing ? "Give the mouse and keyboard back to this Mac" : "Use this Mac's mouse and keyboard in the glasses",
                                  action: #selector(toggleCapture), keyEquivalent: "")
            hold.target = self
            menu.addItem(hold)
            let centre = NSMenuItem(title: "Recentre the view (Ctrl-Option-R)", action: #selector(recentre), keyEquivalent: "")
            centre.target = self
            menu.addItem(centre)
        }
        menu.addItem(.separator())

        buildComputers(menu)
        if room.active {
            let macs = NSMenuItem(title: "Bring a window of this Mac into the glasses", action: nil, keyEquivalent: "")
            let sub = NSMenu()
            if !MacWindows.allowed(ask: false) {
                let ask = NSMenuItem(title: "Allow Screen Recording\u{2026}", action: #selector(allowCapture), keyEquivalent: "")
                ask.target = self
                sub.addItem(ask)
            } else if room.macList.isEmpty {
                let none = NSMenuItem(title: "No windows to show", action: nil, keyEquivalent: "")
                none.isEnabled = false
                sub.addItem(none)
            }
            for (i, info) in room.macList.prefix(30).enumerated() {
                let entry = NSMenuItem(title: info.app + (info.title.isEmpty ? "" : " \u{2014} " + String(info.title.prefix(40))),
                                       action: #selector(bringMacWindow(_:)), keyEquivalent: "")
                entry.target = self
                entry.tag = i
                sub.addItem(entry)
            }
            macs.submenu = sub
            menu.addItem(macs)
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
        let room = Model.shared.room
        guard sender.tag < room.macList.count else { return }
        room.bringMacWindow(room.macList[sender.tag])
    }

    @objc private func toggleCapture() {
        let room = Model.shared.room
        if room.input.capturing { room.input.stop() } else { room.input.start() }
    }
    @objc private func recentre() { Model.shared.room.recentre() }
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
            Model.shared.glassesOn = glassesWanted()
        }
    }

    @objc private func chooseOutput(_ sender: NSMenuItem) {
        Settings.audioOutputUID = sender.representedObject as? String
        AudioOut.shared.applyDevice()
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

if let at = CommandLine.arguments.firstIndex(of: "--selftest-pad") {
    // --selftest-pad <seconds>: watch the game controllers for a while, printing what they say.
    let seconds = Double(CommandLine.arguments.dropFirst(at + 1).first ?? "") ?? 15
    _ = NSApplication.shared
    PadInput.shared.start()
    var last = ""
    let t = Timer(timeInterval: 0.2, repeats: true) { _ in
        let line = PadInput.shared.debugLine
        if line != last { last = line; print("pad: \(line)") }
    }
    RunLoop.main.add(t, forMode: .common)
    DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
        print("pad: \(PadInput.shared.names.count) controllers \(PadInput.shared.names), \(PadTouchpad.shared.reports) touchpad reports from \(PadTouchpad.shared.devices) device(s)")
        exit(0)
    }
    RunLoop.main.run()
}

if CommandLine.arguments.contains("--selftest-display") {
    // --selftest-display: which double-width modes the glasses' display offers at 72 Hz and at 60, asking for each in turn
    // and putting them back in one eye afterwards. Touches nothing but the glasses' own display.
    setvbuf(stdout, nil, _IOLBF, 0)
    _ = NSApplication.shared
    func modes() -> String {
        guard let id = GlassesOutput.findDisplay() else { return "no glasses display" }
        let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        let all = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
        let now = CGDisplayCopyDisplayMode(id)
        return "now \(now?.pixelWidth ?? 0)x\(now?.pixelHeight ?? 0)@\(now?.refreshRate ?? 0); offered " + all.map { "\($0.pixelWidth)x\($0.pixelHeight)@\(Int($0.refreshRate))" }.joined(separator: " ")
    }
    do {
        let device = try XRealDevice()
        print("start: " + modes())
        for (name, mode) in [("3D 72", XRealDevice.DisplayMode.sbs3D72), ("3D 60", .sbs3D60)] {
            let ok = try device.setDisplayMode(mode)
            print("asked for \(name): \(ok)")
            for _ in 0..<4 { RunLoop.main.run(until: Date().addingTimeInterval(2)); print("   ... " + modes().prefix(60)) }
            print("\(name): " + modes())
        }
        let ok = try device.setDisplayMode(.mono1080p60)
        print("asked for one eye: \(ok)")
        for _ in 0..<4 { RunLoop.main.run(until: Date().addingTimeInterval(2)); print("   ... " + modes().prefix(60)) }
        print("back: " + modes())
    } catch {
        print("display: \(error)")
    }
    exit(0)
}

if let at = CommandLine.arguments.firstIndex(of: "--selftest-imu") {
    // --selftest-imu <seconds>: read the glasses' sensors into a room, without touching their display or this
    // Mac's mouse, and say how the tracker is doing.
    let seconds = Double(CommandLine.arguments.dropFirst(at + 1).first ?? "") ?? 30
    _ = NSApplication.shared
    setvbuf(stdout, nil, _IOLBF, 0)
    let core = RoomCore()
    core.setDevice("XREAL Air")
    do {
        let device = try XRealDevice()
        var n = 0
        try device.startIMU { sample in
            core.imu(timestamp: sample.timestamp, gyro: sample.gyro, accel: sample.accel, mag: sample.mag)
            n += 1
            if n % 4000 == 1 {
                print(String(format: "imu raw: t %llu gyro %.3f %.3f %.3f accel %.3f %.3f %.3f mag %.3f %.3f %.3f", sample.timestamp,
                             sample.gyro.x, sample.gyro.y, sample.gyro.z, sample.accel.x, sample.accel.y, sample.accel.z,
                             sample.mag.x, sample.mag.y, sample.mag.z))
            }
        }
        let t = Timer(timeInterval: 5, repeats: true) { _ in
            let h = core.headDegrees
            print(String(format: "head yaw %.2f pitch %.2f roll %.2f", h.yaw, h.pitch, h.roll))
        }
        RunLoop.main.add(t, forMode: .common)
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { device.stopIMU(); exit(0) }
    } catch {
        print("imu: could not open the glasses: \(error)")
        exit(1)
    }
    RunLoop.main.run()
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

if let at = CommandLine.arguments.firstIndex(of: "--selftest-focus") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    FocusTest.run(app: args.first ?? "TextEdit", title: args.dropFirst().first ?? "")
}
if let at = CommandLine.arguments.firstIndex(of: "--selftest-panel") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    PanelTest.run(app: args.first ?? "TextEdit", title: args.dropFirst().first ?? "")
}
if let at = CommandLine.arguments.firstIndex(of: "--selftest-perf") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    PerfTest.run(app: args.first ?? "Claude", title: args.dropFirst().first ?? "Claude")
}
if let at = CommandLine.arguments.firstIndex(of: "--selftest-post") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    PostTest.run(app: args.first ?? "ClickProbe", title: args.dropFirst().first ?? "")
}
if let at = CommandLine.arguments.firstIndex(of: "--selftest-room") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    guard args.count >= 3 else { print("usage: --selftest-room <address> <fingerprint> <app>"); exit(2) }
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async { RoomTest.run(address: args[0], fingerprint: args[1], app: args[2]) }
    application.run()
}

if let at = CommandLine.arguments.firstIndex(of: "--selftest-window") {
    let args = Array(CommandLine.arguments[(at + 1)...])
    guard args.count >= 3 else { print("usage: --selftest-window <address> <fingerprint> <app>"); exit(2) }
    let application = NSApplication.shared
    application.setActivationPolicy(.accessory)
    DispatchQueue.main.async { WindowTest.run(address: args[0], fingerprint: args[1], app: args[2]) }
    application.run()
}

let application = NSApplication.shared
let delegate = App()
application.delegate = delegate
// A menu-bar app: no Dock icon and no main window.
application.setActivationPolicy(.accessory)
application.run()
