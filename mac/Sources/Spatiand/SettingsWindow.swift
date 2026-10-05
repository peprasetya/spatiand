//  SettingsWindow.swift — what the owner can change, opened by Ctrl-Tab or from the menu.

import AppKit
import ServiceManagement
import SwiftUI

struct SettingsView: View {
    @AppStorage("presentation", store: Defaults.store) private var presentation = Presentation.automatic.rawValue
    @AppStorage("audioOutputUID", store: Defaults.store) private var output = ""
    @AppStorage("commandIsControl", store: Defaults.store) private var commandIsControl = true
    @AppStorage("maxKbit", store: Defaults.store) private var maxKbit = 10_000
    @AppStorage("glassesStereo", store: Defaults.store) private var glassesStereo = true
    @AppStorage("captureInput", store: Defaults.store) private var captureInput = true
    @AppStorage("hostControlIsCommand", store: Defaults.store) private var hostControlIsCommand = true
    @AppStorage("hotkeys", store: Defaults.store) private var hotkeys = true
    @AppStorage("gamepads", store: Defaults.store) private var gamepads = true
    @State private var atLogin = SMAppService.mainApp.status == .enabled
    @State private var loginProblem = ""
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
            Toggle("Show the glasses in 3D (applies the next time they are plugged in)", isOn: $glassesStereo)
            Toggle("This Mac's mouse and keyboard steer the glasses when they are on", isOn: $captureInput)
            Text("Ctrl-Option-G gives them back to the Mac at any time.").font(.caption).foregroundColor(.secondary)
            Text("On the trackpad, in the glasses: three fingers swipe sideways to bring the next window here, up for the menu, down to put it away, and tap to recentre. Four fingers swipe to move the window you point at, and pinch to resize it. Five fingers pinch to gather every window in front of you, and spread to give them room.")
                .font(.caption).foregroundColor(.secondary)
            let system = SystemGestures.enabled()
            if !system.isEmpty {
                Text("macOS is also using these on the trackpad: " + system.joined(separator: "; ") + ". They happen on the Mac as well, so turn them off if they get in the way.")
                    .font(.caption).foregroundColor(.orange)
                Button("Open Trackpad settings\u{2026}") {
                    if let url = URL(string: "x-apple.systempreferences:com.apple.Trackpad-Settings") { NSWorkspace.shared.open(url) }
                }
            }
            Toggle("Game controllers work as they do on the Deck (cable or Bluetooth)", isOn: $gamepads)
            Text("A pad's touchpad slides the pointer in the glasses and its press clicks; the PS button opens the menu, which the D-pad, X and O then work. Each window has the controller layout it has on the Deck: copy ~/.config/spatiand/layouts from a Deck into ~/Library/Application Support/Spatiand/layouts to use the same ones.")
                .font(.caption).foregroundColor(.secondary)
            Toggle("Start Spatiand when I log in", isOn: $atLogin)
                .onChange(of: atLogin) { wanted in
                    do {
                        if wanted { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                        loginProblem = ""
                    } catch {
                        loginProblem = "Could not change it: \(error.localizedDescription)"
                        atLogin = SMAppService.mainApp.status == .enabled
                    }
                }
            if !loginProblem.isEmpty { Text(loginProblem).font(.caption).foregroundColor(.red) }
            Toggle("When this Mac is used from another device, its Control key works as Command", isOn: $hostControlIsCommand)
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
