//  NotificationReminder.swift — once, a word about macOS's switch that lets banners through.
//
//  macOS shows no notification banner while a display is mirrored or shared, which Spatiand's
//  virtual screen always counts as: each banner is muted ("muted by display state (displayShared)"
//  in Notification Center's log) and goes unseen to Notification Center's list, so there is no
//  banner for Notices to read, and no Mac notification can be shown in the room or sent to
//  another one. Until the wearer turns on System Settings › Notifications › "Allow notifications
//  when mirroring or sharing the display".
//
//  The owner, 2026-11-18: "I hope there is reminder in the spatiand.. although let user reject it,
//  and not be asked again". So: when banners are seen being muted for that reason, one alert, as
//  the app's other questions are put (an NSAlert on the Mac's screen), saying what is off and why
//  it matters, with a button that opens that pane, and "Don't Remind Me", which is kept
//  (Settings.mirroringReminderDeclined) and never asked again. Not declined and still off: at most
//  once per start of the app. Once the switch is on nothing is muted, and nothing is shown.
//
//  **Found by what is observed, not by reading the switch**: where macOS keeps it is not
//  documented, and no preference domain on macOS 27 holds it (com.apple.ncprefs is gone). So
//  Notification Center's own log is read (`log show`, which an ordinary user may run) for banners
//  muted since the app started, every few minutes until one is found or the reminder is put away.
//  The Settings window keeps its plain line about the switch either way.

import AppKit

enum NotificationReminder {
    /// How often the log is looked at.
    private static let every: TimeInterval = 180
    private static var started = Date()
    private static var done = false
    private static var timer: Timer?

    /// From the app's start: looks for muted banners until it has reminded once or need not.
    static func start() {
        started = Date()
        guard !Settings.mirroringReminderDeclined else { return }
        timer = Timer.scheduledTimer(withTimeInterval: every, repeats: true) { _ in look() }
        // A first look a minute in, once the virtual screen and the room have settled.
        DispatchQueue.main.asyncAfter(deadline: .now() + 60) { look() }
    }

    private static func look() {
        guard !done, !Settings.mirroringReminderDeclined else { stop(); return }
        let minutes = max(1, Int(Date().timeIntervalSince(started) / 60) + 1)
        DispatchQueue.global(qos: .utility).async {
            let muted = mutedSince(minutes: minutes)
            DispatchQueue.main.async {
                if muted && !done && !Settings.mirroringReminderDeclined { remind() }
            }
        }
    }

    private static func stop() {
        timer?.invalidate()
        timer = nil
    }

    /// Whether Notification Center's log has a banner muted because a display is shared in the
    /// last `minutes`.
    static func mutedSince(minutes: Int) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/log")
        process.arguments = ["show", "--last", "\(minutes)m", "--style", "compact", "--predicate",
                             "process == \"NotificationCenter\" AND eventMessage CONTAINS \"muted by display state (displayShared)\""]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return false }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return mutedIn(String(decoding: data, as: UTF8.self))
    }

    /// Whether `log show`'s output has a muted banner: a line of NotificationCenter's saying so
    /// (not `log`'s own line naming the predicate it was given).
    static func mutedIn(_ output: String) -> Bool {
        output.split(separator: "\n").contains { $0.contains("NotificationCenter[") && $0.contains("muted by display state (displayShared)") }
    }

    private static func remind() {
        done = true
        stop()
        print("notifications: banners are muted while the display is shared; reminding once")
        let alert = NSAlert()
        alert.messageText = "Notifications are hidden while Spatiand runs"
        alert.informativeText = "macOS shows no notification banner while a display is mirrored or shared, and Spatiand\u{2019}s virtual screen counts as shared. So notifications from this Mac\u{2019}s apps cannot be shown in the room, nor on another device this Mac is used from.\n\nTurn on System Settings \u{203A} Notifications \u{203A} \u{201C}Allow notifications when mirroring or sharing the display\u{201D}."
        alert.addButton(withTitle: "Open Notifications Settings")
        alert.addButton(withTitle: "Don\u{2019}t Remind Me")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate(ignoringOtherApps: true)
        let answer = alert.runModal()
        if answer == .alertSecondButtonReturn {
            Settings.mirroringReminderDeclined = true
        }
        if answer == .alertFirstButtonReturn,
           let url = URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }
}
