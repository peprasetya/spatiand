//  SettingsWindow.swift — what the owner can change, opened by Ctrl-Tab or from the menu.

import AppKit
import SwiftUI

struct SettingsView: View {
    @AppStorage("presentation", store: Defaults.store) private var presentation = Presentation.automatic.rawValue
    @AppStorage("audioOutputUID", store: Defaults.store) private var output = ""
    @AppStorage("commandIsControl", store: Defaults.store) private var commandIsControl = true
    @AppStorage("maxKbit", store: Defaults.store) private var maxKbit = 10_000
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
            Picker("Video limit", selection: $maxKbit) {
                Text("4 Mbit/s (weak Wi-Fi)").tag(4_000)
                Text("8 Mbit/s").tag(8_000)
                Text("10 Mbit/s").tag(10_000)
                Text("16 Mbit/s").tag(16_000)
                Text("25 Mbit/s").tag(25_000)
                Text("40 Mbit/s (wired)").tag(40_000)
            }
            .onChange(of: maxKbit) { _ in Model.shared.sendBandwidth() }
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
