//
//  SpatiKeyTyping.swift
//  SpatiKey typing an entry into one of this Mac's applications (`SP_TYPE_TEXT`,
//  docs/spatikey.md "Typing into a window").
//
//  The room's other typing into Mac windows goes key by key, by position, with the physical
//  keyboard's modifiers -- right for keys a person presses, wrong for a password: a capital needs
//  a Shift the room would add on its own, and a layout other than US changes the letters. Here
//  each piece of text goes to macOS as the characters themselves (CGEvent's Unicode string), so
//  what arrives is what is in the vault, whatever the layout.
//
//  Off the main thread, after the application has been brought to the front.
//

import AppKit
import CSpatiand

enum SpatiKeyTyping {
    private static let queue = DispatchQueue(label: "spatiand.spatikey.typing", qos: .userInitiated)

    /// Answer `SP_TYPE_TEXT` number `request`: `json` is {"bundle", "steps": [{"text"} | {"key"}]}.
    static func type(json: String, request: UInt32) {
        queue.async {
            let error = typing(json: json)
            if let error {
                error.withCString { sp_typed(request, $0) }
            } else {
                sp_typed(request, nil)
            }
        }
    }

    private static func typing(json: String) -> String? {
        guard let data = json.data(using: .utf8),
              let said = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let bundle = said["bundle"] as? String,
              let steps = said["steps"] as? [[String: Any]]
        else { return "not something to type" }
        guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first else {
            return "that application is not running"
        }
        // To the front, and wait (a little) until it is: keys go to the frontmost application.
        DispatchQueue.main.sync { _ = app.activate() }
        for _ in 0..<25 where !app.isActive {
            usleep(20_000)
        }
        guard app.isActive else { return "that application would not come to the front" }
        guard let source = CGEventSource(stateID: .hidSystemState) else { return "no keyboard to type with" }
        for step in steps {
            if let text = step["text"] as? String {
                post(text: text, source: source)
            } else if let key = step["key"] as? String {
                // Tab is key 48, Return 36, on every layout.
                post(code: key == "tab" ? 48 : 36, source: source)
            }
        }
        return nil
    }

    /// Text as Unicode, a few characters to an event (macOS takes at most 20 UTF-16 units each).
    static func post(text: String, source: CGEventSource) {
        var units = Array(text.utf16)
        defer { units.withUnsafeMutableBufferPointer { $0.update(repeating: 0) } }
        var at = 0
        while at < units.count {
            var end = min(at + 16, units.count)
            // Not between the two halves of a character outside the basic plane (an emoji): each
            // half alone, in an event of its own, arrives as a character that is not one.
            if end < units.count, UTF16.isLeadSurrogate(units[end - 1]) {
                end -= 1
            }
            var piece = Array(units[at..<end])
            for down in [true, false] {
                if let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) {
                    event.flags = []
                    piece.withUnsafeMutableBufferPointer { event.keyboardSetUnicodeString(stringLength: $0.count, unicodeString: $0.baseAddress) }
                    event.post(tap: .cghidEventTap)
                }
            }
            piece.withUnsafeMutableBufferPointer { $0.update(repeating: 0) }
            at = end
            usleep(8_000)
        }
    }

    private static func post(code: CGKeyCode, source: CGEventSource) {
        for down in [true, false] {
            if let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) {
                event.flags = []
                event.post(tap: .cghidEventTap)
            }
        }
        usleep(8_000)
    }
}
