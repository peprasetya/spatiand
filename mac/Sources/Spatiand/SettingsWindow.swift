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
    @AppStorage("notifications", store: Defaults.store) private var notifications = true
    @AppStorage("soundPlacement", store: Defaults.store) private var soundPlacement = true
    @State private var atLogin = SMAppService.mainApp.status == .enabled
    @State private var loginProblem = ""
    let hotkeyStatus: () -> String

    @AppStorage("hostEnabled", store: Defaults.store) private var hostEnabled = false
    @AppStorage("virtualScreen", store: Defaults.store) private var virtualScreen = "1920x1080"

    /// Which page is open; a test may say which to begin on.
    @State private var page = Int(ProcessInfo.processInfo.environment["SPATIAND_SETTINGS_PAGE"] ?? "") ?? 0

    /// The small print under a setting: wrapped, never cut off.
    private func note(_ text: String, colour: Color = .secondary) -> some View {
        Text(text).font(.callout).foregroundColor(colour).fixedSize(horizontal: false, vertical: true)
    }

    var body: some View {
        TabView(selection: $page) {
            glasses.tabItem { Label("Glasses", systemImage: "eyeglasses") }.tag(0)
            control.tabItem { Label("Control", systemImage: "cursorarrow.rays") }.tag(1)
            sound.tabItem { Label("Sound", systemImage: "speaker.wave.2") }.tag(2)
            computers.tabItem { Label("Computers", systemImage: "desktopcomputer") }.tag(3)
            general.tabItem { Label("General", systemImage: "gearshape") }.tag(4)
        }
        .frame(width: 600, height: 470)
    }

    private var glasses: some View {
        Form {
            Section {
                Picker("Show windows", selection: $presentation) {
                    ForEach(Presentation.allCases, id: \.rawValue) { Text($0.title).tag($0.rawValue) }
                }
                Toggle("Show the glasses in 3D", isOn: $glassesStereo)
                note("Applies the next time the glasses are plugged in.")
            }
            Section {
                Toggle("Show this Mac\u{2019}s notifications under the clock", isOn: $notifications)
            }
            Section("Virtual screen") {
                Picker("Size", selection: $virtualScreen) {
                    ForEach(MacScreens.sizes, id: \.name) { Text($0.name).tag("\($0.width)x\($0.height)") }
                }
                .onChange(of: virtualScreen) { _ in MacScreens.forget() }
                note("A screen of its own for this Mac, shown only in the glasses or on a device using this Mac: bring it out from the list of windows, under Screens. macOS takes it for a monitor, so windows can be put on it and it is arranged in System Settings \u{2192} Displays. Changing the size closes it; it is made again when next brought out.")
            }
        }
        .formStyle(.grouped)
    }

    private var control: some View {
        Form {
            Section("Mouse and keyboard") {
                Toggle("Steer the glasses with this Mac\u{2019}s mouse and keyboard", isOn: $captureInput)
                note("Control-Option-G gives them back to the Mac\u{2019}s own screen, and takes them again.")
            }
            Section("Trackpad, in the glasses") {
                LabeledContent("Two fingers", value: "Scroll")
                LabeledContent("Pinch on a title bar", value: "Nearer or further")
                LabeledContent("Three fingers", value: "Drag, as a held button")
                LabeledContent("Four fingers up / down", value: "Launcher / back")
                LabeledContent("Four-finger tap", value: "Recentre")
                let system = SystemGestures.enabled()
                if !system.isEmpty {
                    note("macOS is also using these on the trackpad: " + system.joined(separator: "; ") + ". They happen on the Mac as well, so turn them off if they get in the way.", colour: .orange)
                    Button("Open Trackpad settings\u{2026}") {
                        if let url = URL(string: "x-apple.systempreferences:com.apple.Trackpad-Settings") { NSWorkspace.shared.open(url) }
                    }
                }
            }
            Section("Game controllers") {
                Toggle("Work as they do on the Deck", isOn: $gamepads)
                note("By cable or Bluetooth. A pad\u{2019}s touchpad moves the pointer and its press clicks; the PS button opens the menu. To use a Deck\u{2019}s layouts, copy ~/.config/spatiand/layouts from it to the same place on this Mac.")
            }
        }
        .formStyle(.grouped)
    }

    private var sound: some View {
        Form {
            Section {
                Picker("Sound output", selection: $output) {
                    Text("Same as this Mac").tag("")
                    ForEach(AudioDevices.outputs(), id: \.uid) { Text($0.name).tag($0.uid) }
                }
                .onChange(of: output) { _ in AudioOut.shared.applyDevice() }
            }
            Section {
                Toggle("A Mac application\u{2019}s sound comes from its window", isOn: $soundPlacement)
                note("In the glasses, its sound is taken from the Mac\u{2019}s speakers and placed where its window is. Applies the next time the glasses are plugged in; macOS asks once whether Spatiand may record system audio.")
            }
        }
        .formStyle(.grouped)
    }

    private var computers: some View {
        Form {
            Section("Other computers, used from this Mac") {
                Picker("Video limit", selection: $maxKbit) {
                    Text("4 Mbit/s (weak Wi-Fi)").tag(4_000)
                    Text("8 Mbit/s").tag(8_000)
                    Text("10 Mbit/s").tag(10_000)
                    Text("16 Mbit/s").tag(16_000)
                    Text("25 Mbit/s").tag(25_000)
                    Text("40 Mbit/s (wired)").tag(40_000)
                }
                .onChange(of: maxKbit) { _ in Model.shared.sendBandwidth() }
                Toggle("Command works as Control in their windows", isOn: $commandIsControl)
            }
            Section("This Mac, used from another device") {
                Toggle("Let a paired Deck or Beam Pro use this Mac\u{2019}s windows", isOn: $hostEnabled)
                    .onChange(of: hostEnabled) { on in if on { MacHost.shared.start() } else { MacHost.shared.stop() } }
                if hostEnabled { note("Reached as \(MacHost.shared.address). Pair a device from the Spatiand menu.") }
                Toggle("Its Control key works as Command here", isOn: $hostControlIsCommand)
            }
        }
        .formStyle(.grouped)
    }

    private var general: some View {
        Form {
            Section {
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
                if !loginProblem.isEmpty { note(loginProblem, colour: .red) }
            }
            Section("Keys") {
                Toggle("Control-Space opens the menu, Control-Tab these settings", isOn: $hotkeys)
                if hotkeys { note(hotkeyStatus()) }
            }
        }
        .formStyle(.grouped)
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

    var number: Int { window?.windowNumber ?? 0 }
}
