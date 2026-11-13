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
//  Off the main thread, after the window has been brought to the front. Into that window and no
//  other: the room says which of the Mac's windows is meant, and nothing is posted unless the
//  Mac's frontmost application has that very window as its focused one -- asked of Accessibility
//  before the first piece and again before each one after, since a posted key goes to whatever
//  has the keys at that moment (another window of the same application, a dialog that came up).
//

import AppKit
import CSpatiand

enum SpatiKeyTyping {
    private static let queue = DispatchQueue(label: "spatiand.spatikey.typing", qos: .userInitiated)

    /// Answer `SP_TYPE_TEXT` number `request`: `json` is {"window", "steps": [{"text"} | {"key"}]},
    /// the window by the room's number for it.
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
              let window = (said["window"] as? NSNumber)?.uint32Value,
              let steps = said["steps"] as? [[String: Any]]
        else { return "not something to type" }
        // To the front, the application and that window of it (waited for, briefly, there).
        guard let info = RoomWindows.shared.bringForTyping(window) else { return "that window has gone" }
        // And wait (a little) until the Mac says so itself: keys go to the frontmost
        // application's focused window, whichever that is.
        for _ in 0..<25 where !hasKeys(info) {
            usleep(20_000)
        }
        guard hasKeys(info) else { return "that window would not take the Mac's keyboard" }
        guard let source = CGEventSource(stateID: .hidSystemState) else { return "no keyboard to type with" }
        for (n, step) in steps.enumerated() {
            guard n == 0 || hasKeys(info) else { return "the Mac's keyboard left that window: the rest was not typed" }
            if let text = step["text"] as? String {
                post(text: text, source: source)
            } else if let key = step["key"] as? String {
                // Tab is key 48, Return 36, on every layout.
                post(code: key == "tab" ? 48 : 36, source: source)
            }
        }
        return nil
    }

    /// Whether what is typed now arrives in this window: the Mac's frontmost application is its
    /// own, and that application's focused window is this one (by its window number, read through
    /// Accessibility; without that permission nothing can be told, and nothing is typed).
    private static func hasKeys(_ info: MacWindowInfo) -> Bool {
        MacWindows.frontWindow() == info.windowID
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
