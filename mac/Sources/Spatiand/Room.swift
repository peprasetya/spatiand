//  Room.swift — the room in the glasses: the Deck's compositor, started and fed by the app.
//
//  Everything the wearer sees in the glasses -- the windows and their frames, the launcher, the
//  settings, the pointer, the keyboard -- is drawn and decided by the compositor the Deck and the
//  Beam Pro run (crates/spatiand-mac). This is the app's side of it and deliberately thin:
//
//    * `GlassesScreen` gives it a window on the glasses' display;
//    * `RoomTap` gives it the Mac's mouse, trackpad and keyboard;
//    * `RoomWindows` gives it windows of this Mac's applications, and does to them what it asks;
//    * `PadInput` gives it game controllers;
//    * and here, the list of this Mac's applications for its launcher, the computers this Mac
//      has paired with, and whatever else it asks of a Mac.
//
//  One session a process: when the glasses are unplugged the app starts over (see `main.swift`).

import AppKit
import AVFoundation
import CSpatiand

final class Room {
    static let shared = Room()

    let screen = GlassesScreen()
    private(set) var running = false
    private let sound = MacSound()
    private var soundTimer: Timer?
    var onChange: (() -> Void)?

    var status: String {
        var buffer = [CChar](repeating: 0, count: 256)
        sp_status(&buffer, buffer.count)
        let said = String(cString: buffer)
        return said.isEmpty ? screen.status : said
    }

    /// Start the room: the session, then somewhere for it to draw.
    func start(preview: Bool = false) {
        guard !running else { return }
        running = true
        shareIdentity()
        // Not when it is only being looked at: a host shows its windows to one viewer, and
        // connecting would take them from wherever they are being used.
        if !preview { for host in Hosts.all { sp_paired_host(host.address, host.fingerprint) } }
        sp_display_scale(Double(NSScreen.main?.backingScaleFactor ?? 2))
        sp_microphone_allowed(AVCaptureDevice.authorizationStatus(for: .audio) == .authorized)
        listApplications()
        if let uid = Settings.audioOutputUID, let device = AudioDevices.outputs().first(where: { $0.uid == uid }) {
            sp_audio_output(Int32(bitPattern: device.id))
        }
        sp_begin({ _, what, id, a, b, c, text in
            Room.shared.asked(what, id: id, a: a, b: b, c: c, text: text.map { String(cString: $0) } ?? "")
        }, nil)
        RoomTap.shared.onChange = { [weak self] in self?.onChange?() }
        RoomTap.shared.onRepeat = { RoomWindows.shared.repeated($0) }
        RoomTap.shared.overMacWindow = { RoomWindows.shared.pointerInside }
        screen.onChange = { [weak self] in self?.onChange?() }
        if preview {
            // Nothing is taken from the Mac: it is being looked at, not worn.
            sp_pointer_held(true)
            screen.startPreview()
            if MacWindows.allowed(ask: false) { RoomWindows.shared.start() }
        } else {
            if Settings.captureInput, RoomTap.allowed(ask: true) { RoomTap.shared.hold(true) }
            screen.start()
            // Every window open on this Mac, in the room's list of windows.
            if MacWindows.allowed(ask: true) { RoomWindows.shared.start() }
        }
        PadInput.shared.start()
        // The Mac's notifications, under the clock.
        if Settings.notifications, RoomTap.allowed(ask: false) { Notices.shared.start() }
        // The sound of each application with a window on show, taken and placed at that window.
        if Settings.soundPlacement, !preview {
            sound.onProblem = { print("sound: \($0)") }
            let t = Timer(timeInterval: 1.0, repeats: true) { [weak self] _ in self?.sound.reconcile(wanted: RoomWindows.shared.onShow) }
            RunLoop.main.add(t, forMode: .common)
            soundTimer = t
        }
    }

    /// Give everything back: the display, the pointer, the glasses' mode.
    private var stopping = false

    func stop() {
        guard running, !stopping else { return }
        stopping = true
        defer { stopping = false }
        soundTimer?.invalidate()
        Notices.shared.stop()
        sound.stopAll()
        RoomTap.shared.remove()
        RoomWindows.shared.clear()
        screen.stop()
        sp_end()
        // A moment for the session to put the glasses back in 2D before the process goes.
        let deadline = Date().addingTimeInterval(0.6)
        while sp_running(), Date() < deadline { usleep(20_000) }
        running = false
    }

    // MARK: what the compositor asks

    private func asked(_ what: Int32, id: UInt32, a: Double, b: Double, c: Double, text: String) {
        switch Int(what) {
        case SP_WINDOW_MOTION, SP_WINDOW_LEAVE, SP_WINDOW_BUTTON, SP_WINDOW_SCROLL, SP_WINDOW_KEY, SP_WINDOW_FOCUS,
             SP_WINDOW_RESIZE, SP_WINDOW_CLOSE, SP_WINDOW_SHOWN, SP_WINDOW_HIDDEN:
            RoomWindows.shared.asked(what, id: id, a: a, b: b, c: c)
        case SP_RETURN_TO_DESKTOP:
            DispatchQueue.main.async { RoomTap.shared.hold(false) }
        case SP_LAUNCH:
            DispatchQueue.main.async { self.launch(path: text) }
        case SP_SYSTEM_SETTINGS:
            DispatchQueue.main.async { self.systemSettings(text) }
        case SP_HAPTIC:
            DispatchQueue.main.async {
                NSHapticFeedbackManager.defaultPerformer.perform(a == 2 ? .levelChange : (a == 1 ? .alignment : .generic), performanceTime: .now)
            }
        case SP_NOTICE_PRESSED:
            DispatchQueue.main.async { Notices.shared.pressed() }
        case SP_RECORD_START:
            DispatchQueue.main.async { if #available(macOS 15.0, *) { RoomRecorder.shared.start(screen: self.screen) } else { print("recording: needs macOS 15") } }
        case SP_RECORD_STOP:
            DispatchQueue.main.async { if #available(macOS 15.0, *) { RoomRecorder.shared.stop() } else { print("recording: needs macOS 15") } }
        case SP_SAVED:
            print("saved \(text)")
        default:
            break
        }
    }

    /// Open an application and bring its windows into the room, now and as it opens more.
    func launch(path: String) {
        let url = URL(fileURLWithPath: path)
        guard MacWindows.allowed(ask: true) else { print("launch: Screen Recording has not been allowed"); return }
        let bundle = Bundle(url: url)?.bundleIdentifier ?? ""
        NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration()) { app, error in
            if let error { print("launch: \(url.lastPathComponent): \(error.localizedDescription)"); return }
            let id = app?.bundleIdentifier ?? bundle
            guard !id.isEmpty else { return }
            RoomWindows.shared.follow(bundle: id)
        }
    }

    private func systemSettings(_ pane: String) {
        let address = pane == "bluetooth" ? "x-apple.systempreferences:com.apple.BluetoothSettings" : "x-apple.systempreferences:com.apple.wifi-settings-extension"
        if let url = URL(string: address) { NSWorkspace.shared.open(url) }
        RoomWindows.shared.follow(bundle: "com.apple.systempreferences")
    }

    // MARK: what it is told before it starts

    /// The room connects to hosts as this Mac, with the identity the app paired with: the same
    /// two files, in the place the compositor keeps its own.
    private func shareIdentity() {
        let from = NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
        let to = NSString("~/.config/spatiand").expandingTildeInPath
        try? FileManager.default.createDirectory(atPath: to, withIntermediateDirectories: true)
        for name in ["identity.cert", "identity.key"] {
            let source = from + "/" + name, link = to + "/" + name
            guard FileManager.default.fileExists(atPath: source), !FileManager.default.fileExists(atPath: link) else { continue }
            try? FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: source)
        }
    }

    /// Every application on this Mac, for the launcher: its name, its bundle, and its icon as a
    /// picture in a file, made once.
    private func listApplications() {
        let cache = NSString("~/Library/Caches/Spatiand/icons").expandingTildeInPath
        try? FileManager.default.createDirectory(atPath: cache, withIntermediateDirectories: true)
        var found: [(String, String)] = []
        var seen: Set<String> = []
        let roots = ["/Applications", "/System/Applications", NSString("~/Applications").expandingTildeInPath]
        func look(_ dir: String, depth: Int) {
            guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir) else { return }
            for name in names.sorted() where !name.hasPrefix(".") {
                let path = dir + "/" + name
                if name.hasSuffix(".app") {
                    let title = FileManager.default.displayName(atPath: path).replacingOccurrences(of: ".app", with: "")
                    if seen.insert(title).inserted { found.append((title, path)) }
                } else if depth < 1 {
                    var isDir: ObjCBool = false
                    if FileManager.default.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue { look(path, depth: depth + 1) }
                }
            }
        }
        for root in roots { look(root, depth: 0) }
        sp_apps_begin()
        for (title, path) in found.sorted(by: { $0.0.localizedCaseInsensitiveCompare($1.0) == .orderedAscending }) {
            let key = (Bundle(path: path)?.bundleIdentifier ?? title).replacingOccurrences(of: "/", with: "_")
            let icon = cache + "/" + key + ".png"
            if !FileManager.default.fileExists(atPath: icon) { Self.writeIcon(of: path, to: icon) }
            sp_app(title, path, FileManager.default.fileExists(atPath: icon) ? icon : nil)
        }
        sp_apps_end()
        print("launcher: \(found.count) applications of this Mac")
    }

    private static func writeIcon(of path: String, to file: String) {
        let image = NSWorkspace.shared.icon(forFile: path)
        let side = 128
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        image.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
        NSGraphicsContext.restoreGraphicsState()
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: file))
    }
}
