//! Deciding *how* to send each token: a physical key press or Unicode packets.
//!
//! Pure logic over the [`Layout`] trait, so it can be unit tested with fake keyboard layouts.
//! The Windows backend implements [`Layout`] with `VkKeyScanExW`, `MapVirtualKeyExW` and
//! `ToUnicodeEx`.

use crate::{job::Mode, text::Token};

/// Scan code of the main Enter key.
pub const SCAN_ENTER: u16 = 0x1C;
/// Scan code of the Tab key.
pub const SCAN_TAB: u16 = 0x0F;

/// A physical key press, with the modifiers to hold while pressing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub scan: u16,
    /// The scan code has the `E0` prefix (e.g. right Alt, arrow keys).
    pub extended: bool,
    pub shift: bool,
    pub altgr: bool,
}

impl KeyPress {
    pub const fn plain(scan: u16) -> Self {
        KeyPress {
            scan,
            extended: false,
            shift: false,
            altgr: false,
        }
    }
}

/// What to send for one token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stroke {
    Key(KeyPress),
    /// UTF-16 code units, sent as Unicode packets.
    Unicode(Vec<u16>),
}

/// Keyboard layout queries (one layout, i.e. one Windows `HKL`).
pub trait Layout {
    /// The virtual key and shift state that type `c` (`VkKeyScanExW`). Shift state bits:
    /// 1 = Shift, 2 = Ctrl, 4 = Alt (Ctrl+Alt = AltGr).
    fn vk_for(&self, c: char) -> Option<(u8, u8)>;
    /// The scan code of a virtual key (`MapVirtualKeyExW(.., MAPVK_VK_TO_VSC_EX, ..)`):
    /// `0xE0xx` / `0xE1xx` for extended keys, 0 if there is none.
    fn scan_for(&self, vk: u8) -> u16;
    /// The single character typed by this key with these modifiers (`ToUnicodeEx` without
    /// changing the keyboard state), or `None` for dead keys and multi-character results.
    fn char_for(&self, vk: u8, scan: u16, shift: bool, altgr: bool) -> Option<char>;
}

/// Plans one token. Enter and Tab are always real key presses; characters follow `mode`,
/// and scan-code mode falls back to Unicode for characters the layout cannot type exactly.
pub fn plan(token: Token, mode: Mode, layout: &dyn Layout) -> Stroke {
    match token {
        Token::Enter => Stroke::Key(KeyPress::plain(SCAN_ENTER)),
        Token::Tab => Stroke::Key(KeyPress::plain(SCAN_TAB)),
        Token::Char(c) => match mode {
            Mode::Unicode => unicode(c),
            Mode::Scancode => key_press(c, layout).map_or_else(|| unicode(c), Stroke::Key),
        },
    }
}

pub fn unicode(c: char) -> Stroke {
    let mut buf = [0u16; 2];
    Stroke::Unicode(c.encode_utf16(&mut buf).to_vec())
}

fn key_press(c: char, layout: &dyn Layout) -> Option<KeyPress> {
    // `VkKeyScanExW` takes a single UTF-16 unit.
    if u32::from(c) > 0xFFFF {
        return None;
    }
    let (vk, state) = layout.vk_for(c)?;
    let (shift, altgr) = match state {
        0 => (false, false),
        1 => (true, false),
        6 => (false, true),
        7 => (true, true),
        // Ctrl-only, Alt-only, Kana/OEM modifiers...: not something we can press safely.
        _ => return None,
    };
    let raw = layout.scan_for(vk);
    if raw == 0 {
        return None;
    }
    let extended = matches!(raw >> 8, 0xE0 | 0xE1);
    // Round trip: rejects dead keys (e.g. `^` on German layouts) and inexact mappings.
    if layout.char_for(vk, raw, shift, altgr) != Some(c) {
        return None;
    }
    Some(KeyPress {
        scan: raw & 0xFF,
        extended,
        shift,
        altgr,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A tiny fake layout: char → (vk, shift state), vk → scan code, plus dead keys.
    struct Fake {
        keys: HashMap<char, (u8, u8)>,
        scans: HashMap<u8, u16>,
        dead: Vec<char>,
    }

    impl Fake {
        /// Letters get scan codes 0x10.. in alphabetical order (not the real ones; fine here).
        fn us() -> Self {
            let mut f = Fake {
                keys: HashMap::new(),
                scans: HashMap::new(),
                dead: vec![],
            };
            for (i, c) in ('a'..='z').enumerate() {
                let vk = b'A' + i as u8;
                f.keys.insert(c, (vk, 0));
                f.keys.insert(c.to_ascii_uppercase(), (vk, 1));
                f.scans.insert(vk, 0x10 + i as u16);
            }
            f.keys.insert('1', (b'1', 0));
            f.keys.insert('!', (b'1', 1));
            f.scans.insert(b'1', 0x02);
            f
        }

        fn german() -> Self {
            let mut f = Fake::us();
            f.keys.insert('@', (b'Q', 6)); // AltGr+Q
            f.keys.insert('€', (b'E', 6)); // AltGr+E
            f.keys.insert('^', (0xDC, 0));
            f.scans.insert(0xDC, 0x29);
            f.dead.push('^');
            // A key whose scan code lives on the extended (E0) page.
            f.keys.insert('÷', (0x6F, 0));
            f.scans.insert(0x6F, 0xE035);
            f
        }
    }

    impl Layout for Fake {
        fn vk_for(&self, c: char) -> Option<(u8, u8)> {
            self.keys.get(&c).copied()
        }

        fn scan_for(&self, vk: u8) -> u16 {
            self.scans.get(&vk).copied().unwrap_or(0)
        }

        fn char_for(&self, vk: u8, _scan: u16, shift: bool, altgr: bool) -> Option<char> {
            let state = u8::from(shift) | if altgr { 6 } else { 0 };
            let (&c, _) = self.keys.iter().find(|(_, k)| **k == (vk, state))?;
            (!self.dead.contains(&c)).then_some(c)
        }
    }

    fn key(scan: u16, shift: bool, altgr: bool) -> Stroke {
        Stroke::Key(KeyPress {
            scan,
            extended: false,
            shift,
            altgr,
        })
    }

    #[test]
    fn unicode_mode_sends_utf16_units() {
        let us = Fake::us();
        assert_eq!(
            plan(Token::Char('a'), Mode::Unicode, &us),
            Stroke::Unicode(vec![0x61])
        );
        assert_eq!(
            plan(Token::Char('😀'), Mode::Unicode, &us),
            Stroke::Unicode(vec![0xD83D, 0xDE00])
        );
    }

    #[test]
    fn enter_and_tab_are_keys_in_every_mode() {
        let us = Fake::us();
        for mode in Mode::ALL {
            assert_eq!(plan(Token::Enter, mode, &us), key(SCAN_ENTER, false, false));
            assert_eq!(plan(Token::Tab, mode, &us), key(SCAN_TAB, false, false));
        }
    }

    #[test]
    fn scancode_mode_uses_shift_for_capitals_and_symbols() {
        let us = Fake::us();
        assert_eq!(
            plan(Token::Char('a'), Mode::Scancode, &us),
            key(0x10, false, false)
        );
        assert_eq!(
            plan(Token::Char('A'), Mode::Scancode, &us),
            key(0x10, true, false)
        );
        assert_eq!(
            plan(Token::Char('!'), Mode::Scancode, &us),
            key(0x02, true, false)
        );
    }

    #[test]
    fn scancode_mode_uses_altgr_where_the_layout_needs_it() {
        let de = Fake::german();
        // 'q' is the 17th letter → scan 0x20 in the fake; 'e' the 5th → 0x14.
        assert_eq!(
            plan(Token::Char('@'), Mode::Scancode, &de),
            key(0x20, false, true)
        );
        assert_eq!(
            plan(Token::Char('€'), Mode::Scancode, &de),
            key(0x14, false, true)
        );
    }

    #[test]
    fn extended_scan_codes_are_flagged() {
        let de = Fake::german();
        assert_eq!(
            plan(Token::Char('÷'), Mode::Scancode, &de),
            Stroke::Key(KeyPress {
                scan: 0x35,
                extended: true,
                shift: false,
                altgr: false
            })
        );
    }

    #[test]
    fn scancode_mode_falls_back_to_unicode() {
        let de = Fake::german();
        // dead key
        assert_eq!(plan(Token::Char('^'), Mode::Scancode, &de), unicode('^'));
        // not on the layout at all
        assert_eq!(plan(Token::Char('ß'), Mode::Scancode, &de), unicode('ß'));
        // outside the Basic Multilingual Plane
        assert_eq!(plan(Token::Char('😀'), Mode::Scancode, &de), unicode('😀'));
    }
}
