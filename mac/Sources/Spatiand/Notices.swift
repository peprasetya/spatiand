//  Notices.swift — the Mac's notifications, read so the room can show them.
//
//  macOS gives an application no way to hear of another application's notifications. What it
//  does give, to an application allowed Accessibility, is what is on the screen: a banner is a
//  group of texts in Notification Center's window, and can be pressed. So a banner is found by
//  looking, twice a second, and its words are handed to the compositor, which shows them as a
//  small card under the clock (`sp_notice`); pressing that card presses the banner, which is
//  what opens the application it came from -- and that application's windows are then brought
//  into the room.

import AppKit
import ApplicationServices
import CSpatiand

final class Notices {
    static let shared = Notices()

    private var timer: Timer?
    /// **Looking is done off the main thread**: it asks other applications, over accessibility, for
    /// what they show, and each ask waits for the other application to answer -- as long as it
    /// takes. On the main thread that was the pointer and keys stopping for a good part of a second
    /// at a time. Everything below the timer is touched on this queue only.
    private let queue = DispatchQueue(label: "spatiand.notices", qos: .utility)
    private var looking = false
    private var shown = ""
    private var banner: AXUIElement?

    /// Told of each notification as it comes, and of its going (empty): for a session this Mac
    /// is the host of, whose wearer cannot see this screen.
    var onShown: ((String) -> Void)?

    /// Who the banners are read for. **Each starts and stops its own reading**: the room's
    /// leaving the glasses used to stop the one timer, which was also the host's -- so from the
    /// moment the glasses were unplugged from this Mac (to be plugged into a Deck, say) the Deck
    /// was told of no notification here, though it went on hearing their sound in the screen's
    /// (the owner, 2026-11-17: "i can hear there is notification sound from mac but no visual in
    /// deck").
    enum Reader { case room, host }
    private var readers: Set<Reader> = []

    func start(for reader: Reader) {
        readers.insert(reader)
        guard timer == nil else { return }
        let t = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in
            guard let self, !self.looking else { return }
            self.looking = true
            // **"Show this Mac's notifications" is asked each time, for every reader**: off, no
            // banner is read, none is shown under the clock and none is said to a session this
            // Mac is the host of -- and turning it off or on takes effect at once, not at the
            // room's next start. The card is the room's only while the room is a reader.
            let on = Settings.notifications
            let room = self.readers.contains(.room)
            self.queue.async { self.look(on: on, room: room); DispatchQueue.main.async { self.looking = false } }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop(for reader: Reader) {
        readers.remove(reader)
        // The room's card goes with the room's reading, whoever else still reads.
        if reader == .room { queue.async { [self] in if cardShown { sp_notice("") }; cardShown = false } }
        guard readers.isEmpty else { return }
        timer?.invalidate()
        timer = nil
        queue.async { [self] in
            if cardShown { sp_notice("") }
            cardShown = false
            shown = ""
        }
    }

    private static func children(_ element: AXUIElement) -> [AXUIElement] {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &value) == .success else { return [] }
        return (value as? [AXUIElement]) ?? []
    }

    private static func string(_ element: AXUIElement, _ attribute: String) -> String? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
        return value as? String
    }

    /// The words in an element, in the order they are read: every static text under it.
    private static func words(_ element: AXUIElement, depth: Int = 0, into out: inout [String]) {
        if string(element, kAXRoleAttribute) == (kAXStaticTextRole as String), let text = string(element, kAXValueAttribute), !text.isEmpty {
            out.append(text)
        }
        guard depth < 8 else { return }
        for child in children(element) { words(child, depth: depth + 1, into: &out) }
    }

    /// The banners on show now: each one's element and its words.
    static func banners() -> [(AXUIElement, [String])] {
        guard let center = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.notificationcenterui").first else { return [] }
        let app = AXUIElementCreateApplication(center.processIdentifier)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success, let windows = value as? [AXUIElement] else { return [] }
        var found: [(AXUIElement, [String])] = []
        // A banner is the smallest thing that can be pressed and has words in it.
        func search(_ element: AXUIElement, depth: Int) {
            var actions: CFArray?
            let pressable = AXUIElementCopyActionNames(element, &actions) == .success && ((actions as? [String])?.contains(kAXPressAction as String) ?? false)
            if pressable {
                var texts: [String] = []
                words(element, into: &texts)
                if !texts.isEmpty { found.append((element, texts)); return }
            }
            guard depth < 8 else { return }
            for child in children(element) { search(child, depth: depth + 1) }
        }
        for window in windows { search(window, depth: 0) }
        return found
    }

    /// What Notification Center has on show, as accessibility says it: each element's role, what
    /// it can be asked to do and its words, indented by depth. For `--selftest-notice`, when no
    /// banner is found: how a macOS that arranges its banners differently is looked at.
    static func tree() -> String {
        guard let center = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.notificationcenterui").first else {
            return "Notification Center is not running"
        }
        let app = AXUIElementCreateApplication(center.processIdentifier)
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value)
        guard result == .success, let windows = value as? [AXUIElement] else { return "its windows cannot be read (\(result.rawValue))" }
        var out = "\(windows.count) window(s)\n"
        func walk(_ element: AXUIElement, _ depth: Int) {
            var actions: CFArray?
            AXUIElementCopyActionNames(element, &actions)
            let role = string(element, kAXRoleAttribute) ?? "?"
            let sub = string(element, kAXSubroleAttribute) ?? ""
            let words = string(element, kAXValueAttribute) ?? string(element, kAXTitleAttribute) ?? string(element, kAXDescriptionAttribute) ?? ""
            out += String(repeating: "  ", count: depth) + "\(role) \(sub) \((actions as? [String]) ?? []) \(words.prefix(60))\n"
            guard depth < 9 else { return }
            for child in children(element) { walk(child, depth + 1) }
        }
        for window in windows { walk(window, 1) }
        return out
    }

    /// Banners already seen, so that one that stays on the screen (an alert waiting to be
    /// answered) is shown once and not for as long as it sits there.
    private var seen: Set<String> = []
    private var since = Date.distantPast

    /// Whether the room's card has words on it now (`sp_notice`).
    private var cardShown = false

    private func look(on: Bool, room: Bool) {
        guard on else {
            // Turned off while one was on show: it goes, from the card and from the session.
            if cardShown { sp_notice("") }
            cardShown = false
            if !shown.isEmpty { DispatchQueue.main.async { [self] in onShown?("") } }
            shown = ""
            banner = nil
            return
        }
        if !room, cardShown { sp_notice(""); cardShown = false }
        let all = Self.banners().map { ($0.0, $0.1, $0.1.joined(separator: "\u{1}")) }
        let fresh = all.first { !seen.contains($0.2) }
        seen = Set(all.map { $0.2 })
        if let fresh {
            banner = fresh.0
            // Who it is from, then what it says, on two lines.
            let head = fresh.1.first ?? ""
            let rest = fresh.1.dropFirst().joined(separator: " \u{2014} ")
            shown = rest.isEmpty ? head : head + "\n" + rest
            since = Date()
            if room { sp_notice(shown); cardShown = true }
            let words = shown
            DispatchQueue.main.async { [self] in onShown?(words) }
        } else if !shown.isEmpty, Date().timeIntervalSince(since) > 8 {
            // A glance's worth: the banner itself may stay on the Mac's screen much longer.
            shown = ""
            banner = nil
            if cardShown { sp_notice("") }
            cardShown = false
            DispatchQueue.main.async { [self] in onShown?("") }
        }
    }

    /// The card in the room was pressed: press the banner, and bring what it opens into the room.
    /// `done` (on the main thread) is told whether the press opened an application.
    ///
    /// **Only a press that worked and brought another application forward brings anything in**:
    /// a banner that has gone from the screen refuses the press, and whatever happened to be in
    /// front was taken for what it opened -- its windows brought into the room, or shown to a
    /// session.
    func pressed(_ done: ((Bool) -> Void)? = nil) {
        let before = NSWorkspace.shared.frontmostApplication?.processIdentifier
        queue.async { [self] in
            guard let banner, AXUIElementPerformAction(banner, kAXPressAction as CFString) == .success else {
                DispatchQueue.main.async { done?(false) }
                return
            }
            after(before, done)
        }
    }

    /// What a pressed banner brought forward, for whoever pressed it from somewhere else: this
    /// Mac as a host shows its session that application's windows.
    var onOpened: ((String) -> Void)?

    private func after(_ before: pid_t?, _ done: ((Bool) -> Void)?) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [self] in
            guard let front = NSWorkspace.shared.frontmostApplication, front.processIdentifier != before,
                  let id = front.bundleIdentifier, id != Bundle.main.bundleIdentifier else {
                done?(false)
                return
            }
            if Room.shared.inGlasses { RoomWindows.shared.follow(bundle: id) }
            onOpened?(id)
            done?(true)
        }
    }
}
