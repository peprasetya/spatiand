//  main.swift — Spatiand's menu-bar app.
//
//  The whole interface until something is open is one icon at the top right of the screen. Its
//  menu says what Spatiand is doing, where its windows go, and where its sound plays. Everything
//  else -- the computers it is paired with, the windows it is showing -- will appear in it as the
//  parts that make them are written; today those sections say so.
//
//  `Spatiand --selftest` prints what it can see and exits, for checking without a screen.

import AppKit

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

    func applicationDidFinishLaunching(_ note: Notification) {
        if let button = item.button {
            button.image = NSImage(systemSymbolName: "eyeglasses", accessibilityDescription: "Spatiand")
        }
        let menu = NSMenu()
        menu.delegate = self
        item.menu = menu
        glasses.onChange = { [weak self] _ in self?.refreshIcon() }
        glasses.start()
        refreshIcon()
        Model.shared.glassesOn = glassesWanted()
        pairing.start()
        hotkeys.onMenu = { [weak self] in self?.item.button?.performClick(nil) }
        hotkeys.onSettings = { [weak self] in self?.settings.show() }
        if Settings.hotkeys { hotkeys.enable() }
        Model.shared.onProblem = { PairingUI.alert("Spatiand", $0) }
        // Back to the computer this Mac was last using, if there is one.
        if let last = Hosts.all.first { Model.shared.connect(last) }
    }

    /// The icon says which world Spatiand is in: dimmed with nothing plugged in, normal with
    /// glasses.
    private func refreshIcon() {
        item.button?.appearsDisabled = !glasses.isPluggedIn
        Model.shared.glassesOn = glassesWanted()
    }

    /// Glasses plugged in, and the owner has not said to keep everything on the Mac.
    private func glassesWanted() -> Bool {
        glasses.isPluggedIn && Settings.presentation == .automatic
    }

    // MARK: the menu, rebuilt each time it opens so it is never out of date

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()

        let status = glasses.isPluggedIn
            ? "Glasses connected — windows are in the room"
            : "No glasses — windows are on this Mac"
        add(menu, status, enabled: false)
        menu.addItem(.separator())

        buildComputers(menu)
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

    private func buildComputers(_ menu: NSMenu) {
        let model = Model.shared
        add(menu, "Computers", enabled: false)
        if Hosts.all.isEmpty { add(menu, "   None paired yet", enabled: false) }
        for host in Hosts.all {
            let isCurrent = model.host == host
            let state = isCurrent ? (model.connected ? "  \u{2014} connected" : "  \u{2014} connecting\u{2026}") : ""
            let entry = NSMenuItem(title: "   " + host.name + state, action: nil, keyEquivalent: "")
            let sub = NSMenu()
            if isCurrent {
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

if CommandLine.arguments.contains("--selftest-local") {
    exit(LocalTests.run())
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
