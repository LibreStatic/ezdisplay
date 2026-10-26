//! Android `KeyEvent` key codes to USB HID keyboard-page (0x07) usages.
//!
//! The mapping is by key code (a physical key position); the PC applies its own layout.

/// The HID usage for an Android key code, `None` for keys we do not forward.
pub fn keycode_to_hid(code: i32) -> Option<u16> {
    let usage = match code {
        // KEYCODE_0..9 -> 1..9 then 0.
        7 => 0x27,
        8..=16 => 0x1E + (code - 8) as u16,
        // KEYCODE_DPAD_UP, DOWN, LEFT, RIGHT.
        19 => 0x52,
        20 => 0x51,
        21 => 0x50,
        22 => 0x4F,
        // KEYCODE_A..Z.
        29..=54 => 0x04 + (code - 29) as u16,
        55 => 0x36,  // COMMA
        56 => 0x37,  // PERIOD
        57 => 0xE2,  // ALT_LEFT
        58 => 0xE6,  // ALT_RIGHT
        59 => 0xE1,  // SHIFT_LEFT
        60 => 0xE5,  // SHIFT_RIGHT
        61 => 0x2B,  // TAB
        62 => 0x2C,  // SPACE
        66 => 0x28,  // ENTER
        67 => 0x2A,  // DEL (backspace)
        68 => 0x35,  // GRAVE
        69 => 0x2D,  // MINUS
        70 => 0x2E,  // EQUALS
        71 => 0x2F,  // LEFT_BRACKET
        72 => 0x30,  // RIGHT_BRACKET
        73 => 0x31,  // BACKSLASH
        74 => 0x33,  // SEMICOLON
        75 => 0x34,  // APOSTROPHE
        76 => 0x38,  // SLASH
        82 => 0x65,  // MENU (Application)
        92 => 0x4B,  // PAGE_UP
        93 => 0x4E,  // PAGE_DOWN
        111 => 0x29, // ESCAPE
        112 => 0x4C, // FORWARD_DEL
        113 => 0xE0, // CTRL_LEFT
        114 => 0xE4, // CTRL_RIGHT
        115 => 0x39, // CAPS_LOCK
        116 => 0x47, // SCROLL_LOCK
        117 => 0xE3, // META_LEFT
        118 => 0xE7, // META_RIGHT
        120 => 0x46, // SYSRQ (Print Screen)
        121 => 0x48, // BREAK (Pause)
        122 => 0x4A, // MOVE_HOME
        123 => 0x4D, // MOVE_END
        124 => 0x49, // INSERT
        // KEYCODE_F1..F12.
        131..=142 => 0x3A + (code - 131) as u16,
        143 => 0x53, // NUM_LOCK
        // KEYCODE_NUMPAD_0..9: 0 is 0x62, 1..9 are 0x59..0x61.
        144 => 0x62,
        145..=153 => 0x59 + (code - 145) as u16,
        154 => 0x54, // NUMPAD_DIVIDE
        155 => 0x55, // NUMPAD_MULTIPLY
        156 => 0x56, // NUMPAD_SUBTRACT
        157 => 0x57, // NUMPAD_ADD
        158 => 0x63, // NUMPAD_DOT
        159 => 0x85, // NUMPAD_COMMA
        160 => 0x58, // NUMPAD_ENTER
        161 => 0x67, // NUMPAD_EQUALS
        _ => return None,
    };
    Some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert_eq!(keycode_to_hid(29), Some(0x04)); // A
        assert_eq!(keycode_to_hid(54), Some(0x1D)); // Z
        assert_eq!(keycode_to_hid(8), Some(0x1E)); // 1
        assert_eq!(keycode_to_hid(16), Some(0x26)); // 9
        assert_eq!(keycode_to_hid(7), Some(0x27)); // 0
    }

    #[test]
    fn editing_and_navigation() {
        assert_eq!(keycode_to_hid(66), Some(0x28)); // Enter
        assert_eq!(keycode_to_hid(111), Some(0x29)); // Escape
        assert_eq!(keycode_to_hid(67), Some(0x2A)); // Backspace
        assert_eq!(keycode_to_hid(112), Some(0x4C)); // Delete
        assert_eq!(keycode_to_hid(124), Some(0x49)); // Insert
        assert_eq!(keycode_to_hid(122), Some(0x4A)); // Home
        assert_eq!(keycode_to_hid(123), Some(0x4D)); // End
        assert_eq!(keycode_to_hid(92), Some(0x4B));
        assert_eq!(keycode_to_hid(93), Some(0x4E));
        assert_eq!(keycode_to_hid(19), Some(0x52)); // Up
        assert_eq!(keycode_to_hid(20), Some(0x51)); // Down
        assert_eq!(keycode_to_hid(21), Some(0x50)); // Left
        assert_eq!(keycode_to_hid(22), Some(0x4F)); // Right
        assert_eq!(keycode_to_hid(82), Some(0x65)); // Menu
    }

    #[test]
    fn function_and_system_keys() {
        assert_eq!(keycode_to_hid(131), Some(0x3A)); // F1
        assert_eq!(keycode_to_hid(142), Some(0x45)); // F12
        assert_eq!(keycode_to_hid(120), Some(0x46)); // Print Screen
        assert_eq!(keycode_to_hid(116), Some(0x47));
        assert_eq!(keycode_to_hid(121), Some(0x48));
        assert_eq!(keycode_to_hid(115), Some(0x39)); // Caps Lock
    }

    #[test]
    fn punctuation() {
        let pairs = [
            (69, 0x2D),
            (70, 0x2E),
            (71, 0x2F),
            (72, 0x30),
            (73, 0x31),
            (74, 0x33),
            (75, 0x34),
            (68, 0x35),
            (55, 0x36),
            (56, 0x37),
            (76, 0x38),
            (61, 0x2B),
            (62, 0x2C),
        ];
        for (code, usage) in pairs {
            assert_eq!(keycode_to_hid(code), Some(usage), "code {code}");
        }
    }

    #[test]
    fn modifiers() {
        let pairs = [
            (113, 0xE0),
            (59, 0xE1),
            (57, 0xE2),
            (117, 0xE3),
            (114, 0xE4),
            (60, 0xE5),
            (58, 0xE6),
            (118, 0xE7),
        ];
        for (code, usage) in pairs {
            assert_eq!(keycode_to_hid(code), Some(usage), "code {code}");
        }
    }

    #[test]
    fn numpad() {
        assert_eq!(keycode_to_hid(143), Some(0x53)); // Num Lock
        assert_eq!(keycode_to_hid(144), Some(0x62)); // 0
        assert_eq!(keycode_to_hid(145), Some(0x59)); // 1
        assert_eq!(keycode_to_hid(153), Some(0x61)); // 9
        assert_eq!(keycode_to_hid(154), Some(0x54));
        assert_eq!(keycode_to_hid(155), Some(0x55));
        assert_eq!(keycode_to_hid(156), Some(0x56));
        assert_eq!(keycode_to_hid(157), Some(0x57));
        assert_eq!(keycode_to_hid(158), Some(0x63));
        assert_eq!(keycode_to_hid(160), Some(0x58));
    }

    #[test]
    fn unknown_codes_are_none() {
        // UNKNOWN, SOFT_LEFT, HOME, BACK, volume up, power, media play, app switch, negative.
        for code in [0, 1, 3, 4, 24, 26, 85, 187, -1, 100_000] {
            assert_eq!(keycode_to_hid(code), None, "code {code}");
        }
    }

    #[test]
    fn usages_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for code in 0..400 {
            if let Some(u) = keycode_to_hid(code) {
                assert!(seen.insert(u), "usage {u:#x} mapped twice (code {code})");
            }
        }
    }
}
