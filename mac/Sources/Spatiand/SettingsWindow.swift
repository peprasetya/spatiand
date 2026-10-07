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
    @AppStorage("steadyView", store: Defaults.store) private var steadyView = true
    @AppStorage("hostControlIsCommand", store: Defaults.store) private var hostControlIsCommand = true
    @AppStorage("hotkeys", store: Defaults.store) private var hotkeys = true
    @AppStorage("gamepads", store: Defaults.store) private var gamepads = true
    @AppStorage("placeMacSound", store: Defaults.store) private var placeMacSound = false
    @State private var atLogin = SMAppService.mainApp.status == .enabled
    @State private var loginProblem = ""
    @StateObject private var panoramas = PanoramaFolder()
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
            Toggle("Hold the view steady, firmly while typing, so a head that is never quite still does not shake the text", isOn: $steadyView)
            Toggle("This Mac's mouse and keyboard steer the glasses when they are on", isOn: $captureInput)
            Text("Ctrl-Option-G gives them back to the Mac at any time.").font(.caption).foregroundColor(.secondary)
            if !CGPreflightListenEventAccess() {
                Text("The glasses' menus hear the arrow keys of a Mac application in front only with Input Monitoring allowed.").font(.caption).foregroundColor(.orange)
                Button("Allow Input Monitoring\u{2026}") {
                    CGRequestListenEventAccess()
                    if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent") { NSWorkspace.shared.open(url) }
                }
            }
            Text("In the glasses it works as on the Deck. Ctrl-Space opens the launcher and Ctrl-Tab the settings; the arrows and Return work them, or point and click. Drag a window by its title bar, and by the edges of its frame to resize it; the buttons on the bar close it, put it away, pin it to the glass and, when it makes a sound, mute it. On the trackpad, pinch over a window's frame to bring it nearer or push it further, and over its contents to zoom them. Three fingers swipe sideways for the next window, up for the launcher, down to put it away, and tap to recentre; four fingers swipe to move the window you point at and pinch to bring it nearer; five fingers pinch to gather every window and spread to give them room.")
                .font(.caption).foregroundColor(.secondary)
            let system = SystemGestures.enabled()
            if !system.isEmpty {
                Text("macOS is also using these on the trackpad: " + system.joined(separator: "; ") + ". They happen on the Mac as well, so turn them off if they get in the way.")
                    .font(.caption).foregroundColor(.orange)
                Button("Open Trackpad settings\u{2026}") {
                    if let url = URL(string: "x-apple.systempreferences:com.apple.Trackpad-Settings") { NSWorkspace.shared.open(url) }
                }
            }
            Toggle("Put the sound of Mac applications where their windows are in the room, as on the Deck", isOn: $placeMacSound)
            Text("Spatiand takes an application's sound from the Mac's speakers and plays it from its window, turning with your head. macOS asks once whether Spatiand may take other applications' sound; the application is silent on the Mac itself while it is taken. Turn this off and it is back on the speakers.")
                .font(.caption).foregroundColor(.secondary)
            Text("Environment").font(.headline)
            Text(panoramas.summary).font(.caption).foregroundColor(.secondary)
            HStack {
                Button("Show the folder") { panoramas.reveal() }
                Button(panoramas.fetching ? "Fetching\u{2026}" : "Get the default panoramas") { panoramas.fetch() }.disabled(panoramas.fetching || !panoramas.canFetch)
            }
            if !panoramas.progress.isEmpty { Text(panoramas.progress).font(.caption).foregroundColor(.secondary) }
            Text("Choose what surrounds you in the glasses' settings list, under Environment: black, the generated studio, or any picture in this folder, or add one from anywhere with Add an image. The folder is the Deck's own (~/.local/share/spatiand/environments), so a folder of panoramas can be copied across. A name with 180 in it is read as the front half only; _ou or _sbs in it, or a square or very wide picture, as a stereo pair.")
                .font(.caption).foregroundColor(.secondary)
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

/// The folder of environment images, for the settings window: how many there are, where, and the Deck's own script
/// for fetching the default ones, which is run only when the wearer presses the button.
final class PanoramaFolder: ObservableObject {
    @Published var summary = ""
    @Published var progress = ""
    @Published var fetching = false
    private let folder = RoomCore.environmentFolder
    private var script: String? { Bundle.main.path(forResource: "fetch-environments", ofType: "sh") }
    var canFetch: Bool { script != nil && folder != nil }

    init() { refresh() }

    func refresh() {
        let count = folder.flatMap { try? FileManager.default.contentsOfDirectory(atPath: $0) }?.filter { ["jpg", "jpeg", "png"].contains(($0 as NSString).pathExtension.lowercased()) }.count ?? 0
        summary = count == 0 ? "No panoramas yet in \(folder ?? "the folder"). The studio and black are always there." : "\(count) panorama\(count == 1 ? "" : "s") in \(folder ?? "the folder")."
    }

    func reveal() {
        guard let folder else { return }
        try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
        NSWorkspace.shared.open(URL(fileURLWithPath: folder))
    }

    func fetch() {
        guard let script, let folder, !fetching else { return }
        fetching = true
        progress = "Downloading from NOIRLab; each is a few megabytes."
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let task = Process()
            task.executableURL = URL(fileURLWithPath: "/bin/bash")
            task.arguments = [script]
            var env = ProcessInfo.processInfo.environment
            env["SPATIAND_ENVIRONMENTS"] = folder
            task.environment = env
            let out = Pipe()
            task.standardOutput = out
            task.standardError = out
            try? task.run()
            let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            task.waitUntilExit()
            DispatchQueue.main.async {
                self?.fetching = false
                self?.progress = task.terminationStatus == 0 ? "Done." : "It did not finish: " + (text.split(separator: "\n").last.map(String.init) ?? "no answer")
                self?.refresh()
            }
        }
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
