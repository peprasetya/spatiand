//! A Mac's key codes as the evdev codes the compositor's keyboard speaks, and back.
//!
//! Positions, not characters: `kVK_ANSI_A` is the key where A is on a US keyboard, whatever the
//! layout prints on it, and `KEY_A` is the same key. The layout is applied by whoever ends up
//! with the key -- a host's own keymap, or this Mac's when the window is one of its own.

/// (macOS virtual key code, evdev code).
const TABLE: &[(u16, u32)] = &[
    (0x00, 30), (0x0B, 48), (0x08, 46), (0x02, 32), (0x0E, 18), (0x03, 33), (0x05, 34), (0x04, 35),
    (0x22, 23), (0x26, 36), (0x28, 37), (0x25, 38), (0x2E, 50), (0x2D, 49), (0x1F, 24), (0x23, 25),
    (0x0C, 16), (0x0F, 19), (0x01, 31), (0x11, 20), (0x20, 22), (0x09, 47), (0x0D, 17), (0x07, 45),
    (0x10, 21), (0x06, 44),
    // The number row.
    (0x12, 2), (0x13, 3), (0x14, 4), (0x15, 5), (0x17, 6), (0x16, 7), (0x1A, 8), (0x1C, 9),
    (0x19, 10), (0x1D, 11),
    // Punctuation.
    (0x1B, 12), (0x18, 13), (0x21, 26), (0x1E, 27), (0x2A, 43), (0x29, 39), (0x27, 40), (0x32, 41),
    (0x2B, 51), (0x2F, 52), (0x2C, 53), (0x0A, 86),
    // Editing and whitespace.
    (0x24, 28), (0x30, 15), (0x31, 57), (0x33, 14), (0x35, 1), (0x75, 111), (0x72, 110),
    (0x73, 102), (0x77, 107), (0x74, 104), (0x79, 109),
    (0x7B, 105), (0x7C, 106), (0x7D, 108), (0x7E, 103),
    // Modifiers. Command is the "super" key, where a PC keyboard has it.
    (0x38, 42), (0x3C, 54), (0x3B, 29), (0x3E, 97), (0x3A, 56), (0x3D, 100), (0x37, 125),
    (0x36, 126), (0x39, 58),
    // Function keys.
    (0x7A, 59), (0x78, 60), (0x63, 61), (0x76, 62), (0x60, 63), (0x61, 64), (0x62, 65), (0x64, 66),
    (0x65, 67), (0x6D, 68), (0x67, 87), (0x6F, 88),
    // The keypad.
    (0x52, 82), (0x53, 79), (0x54, 80), (0x55, 81), (0x56, 75), (0x57, 76), (0x58, 77), (0x59, 71),
    (0x5B, 72), (0x5C, 73), (0x41, 83), (0x43, 55), (0x45, 78), (0x4B, 98), (0x4C, 96), (0x4E, 74),
    (0x51, 117), (0x47, 69),
];

/// The evdev code for a Mac key.
pub fn evdev_of(mac: u16) -> Option<u32> {
    TABLE.iter().find(|(m, _)| *m == mac).map(|(_, e)| *e)
}

/// The Mac key for an evdev code.
pub fn mac_of(evdev: u32) -> Option<u16> {
    TABLE.iter().find(|(_, e)| *e == evdev).map(|(m, _)| *m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_is_listed_twice_either_way() {
        for (i, (mac, evdev)) in TABLE.iter().enumerate() {
            for (other_mac, other_evdev) in &TABLE[i + 1..] {
                assert_ne!(mac, other_mac, "Mac key {mac:#x} twice");
                assert_ne!(evdev, other_evdev, "evdev {evdev} twice");
            }
        }
    }

    #[test]
    fn the_keys_everything_depends_on_are_where_they_should_be() {
        assert_eq!(evdev_of(0x00), Some(30)); // A
        assert_eq!(evdev_of(0x24), Some(28)); // Return
        assert_eq!(evdev_of(0x33), Some(14)); // Delete is backspace
        assert_eq!(evdev_of(0x37), Some(125)); // Command is super
        assert_eq!(mac_of(29), Some(0x3B)); // Control
        assert_eq!(mac_of(46), Some(0x08)); // C
    }
}
