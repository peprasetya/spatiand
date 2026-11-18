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
//  muted since the previous look (never before the app started): every 3 minutes, every 30 after
//  ten looks found nothing, until one is found or the reminder is put away. Only a banner muted
//  in the latest look reminds, so one from before the switch was turned on does not.
//  The Settings window keeps its plain line about the switch either way.
//
//  With the glasses on (Room.inGlasses) the alert would be on a screen the wearer does not see:
//  the reminder is the room's notice card instead (sp_reminder), the card OR the alert, never
//  both. A card has one press: it opens the pane, on the Mac's screen shown in the room, and
//  "Don't Remind Me" is the Settings window's "Remind me about this" (the same
//  mirroringReminderDeclined).

import AppKit
import CSpatiand

enum NotificationReminder {
    /// How often the log is looked at at first, and after this many looks that found nothing.
    static let every: TimeInterval = 180
    static let backedOff: TimeInterval = 1800
    static let cleanBeforeBackingOff = 10
    /// How far a look reaches back before the previous one, so that nothing falls between two.
    static let margin: TimeInterval = 15
    private static var started = Date()
    private static var lastLook: Date?
    private static var clean = 0
    private static var done = false
    private static var timer: Timer?

    /// From the app's start: looks for muted banners until it has reminded once or need not.
    static func start() {
        started = Date()
        guard !Settings.mirroringReminderDeclined else { return }
        // A first look a minute in, once the virtual screen and the room have settled.
        schedule(after: 60)
    }

    /// Where a look begins: only the time since the previous look (with a small margin), never
    /// before the app's start. So a banner muted before the wearer turned the switch on, or
    /// before the app was started, does not bring the reminder, and a look never scans more
    /// than one interval of the log.
    static func lookStart(previous: Date?, started: Date) -> Date {
        guard let previous else { return started }
        return max(started, previous.addingTimeInterval(-margin))
    }

    /// How long until the next look, after `clean` looks in a row that found nothing.
    static func delay(afterClean clean: Int) -> TimeInterval {
        clean < cleanBeforeBackingOff ? every : backedOff
    }

    private static func schedule(after seconds: TimeInterval) {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { _ in look() }
    }

    private static func look() {
        guard !done, !Settings.mirroringReminderDeclined else { stop(); return }
        let now = Date()
        let from = lookStart(previous: lastLook, started: started)
        lastLook = now
        DispatchQueue.global(qos: .utility).async {
            let muted = muted(from: from)
            DispatchQueue.main.async {
                guard !done, !Settings.mirroringReminderDeclined else { stop(); return }
                if muted { remind(); return }
                clean += 1
                schedule(after: delay(afterClean: clean))
            }
        }
    }

    private static func stop() {
        timer?.invalidate()
        timer = nil
    }

    /// `log show --start`'s form of a moment: local time, to the second.
    static func logTime(_ date: Date) -> String {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.timeZone = .current
        format.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return format.string(from: date)
    }

    /// Whether Notification Center's log has a banner muted because a display is shared since
    /// `from`.
    static func muted(from: Date) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/log")
        process.arguments = ["show", "--start", logTime(from), "--style", "compact", "--predicate",
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

    /// System Settings' Notifications pane, on the Mac's screen (which the room shows).
    static func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }

    private static func remind() {
        done = true
        stop()
        print("notifications: banners are muted while the display is shared; reminding once")
        if Room.shared.inGlasses {
            // The wearer is in the room and would not see an alert on the Mac's screen: the
            // room's card instead, pressed to open the pane. "Don't remind me" is the Settings
            // window's "Remind me about this".
            sp_reminder("Spatiand\nNotifications are hidden: press to open Notifications settings and turn on \u{201C}Allow notifications when mirroring or sharing the display\u{201D}")
            return
        }
        let alert = NSAlert()
        alert.messageText = "Notifications are hidden while Spatiand runs"
        alert.informativeText = "macOS shows no notification banner while a display is mirrored or shared, and Spatiand\u{2019}s virtual screen counts as shared. So notifications from this Mac\u{2019}s apps cannot be shown in the room, nor on another device this Mac is used from.\n\nTurn on System Settings \u{203A} Notifications \u{203A} \u{201C}Allow notifications when mirroring or sharing the display\u{201D}."
        let open = alert.addButton(withTitle: "Open Notifications Settings")
        alert.addButton(withTitle: "Don\u{2019}t Remind Me")
        let notNow = alert.addButton(withTitle: "Not Now")
        // Return answers Not Now: a Return typed for something else opens nothing.
        open.keyEquivalent = ""
        notNow.keyEquivalent = "\r"
        NSApp.activate(ignoringOtherApps: true)
        let answer = alert.runModal()
        if answer == .alertSecondButtonReturn {
            Settings.mirroringReminderDeclined = true
        }
        if answer == .alertFirstButtonReturn { openSettings() }
    }
}
