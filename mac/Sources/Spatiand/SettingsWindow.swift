//  SettingsWindow.swift — what the owner can change, opened by Ctrl-Tab or from the menu.

import AppKit
import SwiftUI

struct SettingsView: View {
    @AppStorage("presentation", store: Defaults.store) private var presentation = Presentation.automatic.rawValue
    @AppStorage("audioOutputUID", store: Defaults.store) private var output = ""
    @AppStorage("commandIsControl", store: Defaults.store) private var commandIsControl = true
    @AppStorage("hotkeys", store: Defaults.store) private var hotkeys = true
    let hotkeyStatus: () -> String

    var body: some View {
        Form {
            Picker("Show windows", selection: $presentation) {
                ForEach(Presentation.allCases, id: \.rawValue) { Text($0.title).tag($0.rawValue) }
            }
            Picker("Sound output", selection: $output) {
                Text("Same as this Mac").tag("")
                ForEach(AudioDevices.outputs(), id: \.uid) { Text($0.name).tag($0.uid) }
            }
            .onChange(of: output) { _ in AudioOut.shared.applyDevice() }
            Toggle("Command works as Control in remote windows", isOn: $commandIsControl)
            Toggle("Ctrl-Space opens the menu, Ctrl-Tab opens these settings", isOn: $hotkeys)
            if hotkeys {
                Text(hotkeyStatus()).font(.caption).foregroundColor(.secondary)
            }
        }
        .padding(20)
        .frame(width: 520)
    }
}

final class SettingsWindow {
    private var window: NSWindow?
    private let hotkeyStatus: () -> String

    init(hotkeyStatus: @escaping () -> String) { self.hotkeyStatus = hotkeyStatus }

    func show() {
        if window == nil {
            let host = NSHostingController(rootView: SettingsView(hotkeyStatus: hotkeyStatus))
            let w = NSWindow(contentViewController: host)
            w.title = "Spatiand Settings"
            w.styleMask = [.titled, .closable]
            w.isReleasedWhenClosed = false
            window = w
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
    }
}
