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
    private var shown = ""
    private var banner: AXUIElement?

    /// Told of each notification as it comes, and of its going (empty): for a session this Mac
    /// is the host of, whose wearer cannot see this screen.
    var onShown: ((String) -> Void)?

    func start() {
        guard timer == nil else { return }
        let t = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in self?.look() }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        if !shown.isEmpty { sp_notice("") }
        shown = ""
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

    /// Banners already seen, so that one that stays on the screen (an alert waiting to be
    /// answered) is shown once and not for as long as it sits there.
    private var seen: Set<String> = []
    private var since = Date.distantPast

    private func look() {
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
            sp_notice(shown)
            onShown?(shown)
        } else if !shown.isEmpty, Date().timeIntervalSince(since) > 8 {
            // A glance's worth: the banner itself may stay on the Mac's screen much longer.
            shown = ""
            banner = nil
            sp_notice("")
            onShown?("")
        }
    }

    /// The card in the room was pressed: press the banner, and bring what it opens into the room.
    func pressed() {
        guard let banner else { return }
        AXUIElementPerformAction(banner, kAXPressAction as CFString)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            if let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier, front != Bundle.main.bundleIdentifier {
                RoomWindows.shared.follow(bundle: front)
            }
        }
    }
}
