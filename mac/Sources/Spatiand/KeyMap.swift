//  KeyMap.swift — a Mac key, as the evdev code a host expects.
//
//  The host takes keys the way a keyboard reports them, before any layout is applied, because
//  the layout lives with the application. So this is a table of *positions*: the physical key
//  that a Mac calls 0x00 is the one Linux calls KEY_A.

import Carbon.HIToolbox

enum KeyMap {
    /// **On an ISO keyboard (a British or German one) macOS reports two keys the other way
    /// about**: the key left of 1 as 0x0A and the key beside left Shift as 0x32, where on an
    /// ANSI keyboard 0x32 is the key left of 1 and 0x0A is not there at all (Linux's hid-apple
    /// puts the same pair right, `iso_layout`). The tables here and in the compositor
    /// (crates/spatiand-mac, keycodes.rs) are of places, and are ANSI's: so with an ISO keyboard
    /// the two are exchanged on the way in, and again on the way out to a window of this Mac's,
    /// whose layout expects them as macOS gives them. Its own inverse; every other key is itself.
    ///
    /// Not tried on an ISO keyboard: none was here (2026-11-11).
    static func place(_ code: UInt16) -> UInt16 {
        guard code == 0x0A || code == 0x32, isISO else { return code }
        return code == 0x0A ? 0x32 : 0x0A
    }

    /// Whether the keyboard last typed on is an ISO one, as macOS has it.
    static var isISO: Bool { KBGetLayoutType(Int16(LMGetKbdType())) == PhysicalKeyboardLayoutType(kKeyboardISO) }

    static let evdev: [UInt16: UInt32] = [
        0: 30, 1: 31, 2: 32, 3: 33, 4: 35, 5: 34, 6: 44, 7: 45, 8: 46, 9: 47, 10: 86, 11: 48,
        12: 16, 13: 17, 14: 18, 15: 19, 16: 21, 17: 20, 18: 2, 19: 3, 20: 4, 21: 5, 22: 7, 23: 6,
        24: 13, 25: 10, 26: 8, 27: 12, 28: 9, 29: 11, 30: 27, 31: 24, 32: 22, 33: 26, 34: 23,
        35: 25, 36: 28, 37: 38, 38: 36, 39: 40, 40: 37, 41: 39, 42: 43, 43: 51, 44: 53, 45: 49,
        46: 50, 47: 52, 48: 15, 49: 57, 50: 41, 51: 14, 53: 1,
        // Modifiers; see `modifier` for how a press is told from a release.
        54: 126, 55: 125, 56: 42, 57: 58, 58: 56, 59: 29, 60: 54, 61: 100, 62: 97,
        // Keypad.
        65: 83, 67: 55, 69: 78, 71: 69, 75: 98, 76: 96, 78: 74, 81: 117, 82: 82, 83: 79, 84: 80,
        85: 81, 86: 75, 87: 76, 88: 77, 89: 71, 91: 72, 92: 73,
        // Function and navigation.
        96: 63, 97: 64, 98: 65, 99: 61, 100: 66, 101: 67, 103: 87, 109: 68, 111: 88, 118: 62,
        120: 60, 122: 59, 115: 102, 116: 104, 117: 111, 119: 107, 121: 109,
        123: 105, 124: 106, 125: 108, 126: 103,
    ]

    /// Which modifier flag a modifier key's code toggles, by mask, so a `flagsChanged` event
    /// can be turned into a press or a release.
    static func modifierMask(_ code: UInt16) -> UInt? {
        switch code {
        case 54, 55: return 1 << 20   // command
        case 56, 60: return 1 << 17   // shift
        case 58, 61: return 1 << 19   // option
        case 59, 62: return 1 << 18   // control
        case 57: return 1 << 16       // caps lock
        default: return nil
        }
    }

    /// The code to send, with Command standing in for Control: a Mac user's copy, paste and
    /// select-all are Command, and on the other end they are Control.
    static func code(for key: UInt16, commandIsControl: Bool) -> UInt32? {
        guard let code = evdev[place(key)] else { return nil }
        if commandIsControl {
            if key == 55 { return 29 }
            if key == 54 { return 97 }
        }
        return code
    }
}
