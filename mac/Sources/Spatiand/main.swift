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
    }

    /// The icon says which world Spatiand is in: dimmed with nothing plugged in, normal with
    /// glasses.
    private func refreshIcon() {
        item.button?.appearsDisabled = !glasses.isPluggedIn
    }

    // MARK: the menu, rebuilt each time it opens so it is never out of date

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()

        let status = glasses.isPluggedIn
            ? "Glasses connected — windows are in the room"
            : "No glasses — windows are on this Mac"
        add(menu, status, enabled: false)
        menu.addItem(.separator())

        // Not built yet. Said plainly rather than left out, so the menu shows the shape it is
        // going to have.
        add(menu, "Computers", enabled: false)
        add(menu, "   None paired yet", enabled: false)
        add(menu, "Windows", enabled: false)
        add(menu, "   Nothing open", enabled: false)
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
        let quit = NSMenuItem(title: "Quit Spatiand", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        menu.addItem(quit)
    }

    private func add(_ menu: NSMenu, _ title: String, enabled: Bool) {
        let entry = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        entry.isEnabled = enabled
        menu.addItem(entry)
    }

    @objc private func choosePresentation(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let choice = Presentation(rawValue: raw) {
            Settings.presentation = choice
        }
    }

    @objc private func chooseOutput(_ sender: NSMenuItem) {
        Settings.audioOutputUID = sender.representedObject as? String
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

let application = NSApplication.shared
let delegate = App()
application.delegate = delegate
// A menu-bar app: no Dock icon and no main window.
application.setActivationPolicy(.accessory)
application.run()
