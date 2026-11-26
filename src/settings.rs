//! User settings, stored as TOML in the config directory.
//!
//! The text to type is never stored: only how to type it.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{job::Mode, paths};

pub const MAX_KEY_DELAY_MS: u32 = 1000;
pub const MAX_COUNTDOWN_SECS: u32 = 120;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Shift+V";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Seconds to wait after Start in the window.
    pub countdown_secs: u32,
    /// Seconds to wait for "type clipboard" (Start menu shortcut, `--clipboard`).
    pub clipboard_delay_secs: u32,
    /// Pause between keystrokes.
    pub key_delay_ms: u32,
    pub mode: Mode,
    /// Drop one trailing line break (copied passwords often end with one).
    pub trim_trailing_newline: bool,
    /// Press Enter after the text.
    pub press_enter: bool,
    /// Minimize the window on Start, so the focus goes back to the previous window.
    pub minimize_on_start: bool,
    /// Bring the window back when typing ends.
    pub restore_after_typing: bool,
    pub abort_on_mouse_move: bool,
    pub abort_on_focus_change: bool,
    /// Clear the clipboard after typing it (type-clipboard actions only).
    pub clear_clipboard_after: bool,
    /// Keep a tray icon running in the background (`--background`).
    pub tray: bool,
    /// Global hotkey of the tray icon: types the clipboard.
    pub hotkey: String,
    pub hotkey_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            countdown_secs: 3,
            clipboard_delay_secs: 2,
            key_delay_ms: 20,
            mode: Mode::Unicode,
            trim_trailing_newline: true,
            press_enter: false,
            minimize_on_start: true,
            restore_after_typing: false,
            abort_on_mouse_move: true,
            abort_on_focus_change: true,
            clear_clipboard_after: false,
            tray: false,
            hotkey: DEFAULT_HOTKEY.into(),
            hotkey_enabled: true,
        }
    }
}

impl Settings {
    /// Keeps values in sane ranges (the file is hand-editable).
    pub fn sanitized(mut self) -> Self {
        self.key_delay_ms = self.key_delay_ms.min(MAX_KEY_DELAY_MS);
        self.countdown_secs = self.countdown_secs.min(MAX_COUNTDOWN_SECS);
        self.clipboard_delay_secs = self.clipboard_delay_secs.min(MAX_COUNTDOWN_SECS);
        if self.hotkey.trim().is_empty() {
            self.hotkey = DEFAULT_HOTKEY.into();
        }
        self
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str::<Settings>(text).map(Settings::sanitized)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("settings always serialize")
    }

    /// Writes the settings atomically (temp file + rename).
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml())?;
        fs::rename(&tmp, path)
    }

    pub fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => Settings::from_toml(&text).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = paths::settings_file()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        self.save_to(&path)
    }
}

/// Loads the settings from the default location, falling back to defaults.
pub fn load() -> Settings {
    paths::settings_file()
        .map(|p| Settings::load_from(&p))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            key_delay_ms: 55,
            mode: Mode::Scancode,
            tray: true,
            ..Settings::default()
        };
        assert_eq!(Settings::from_toml(&s.to_toml()).unwrap(), s);
    }

    #[test]
    fn missing_keys_use_defaults_and_unknown_keys_are_ignored() {
        let s = Settings::from_toml("key_delay_ms = 7\nsomething_new = true\n").unwrap();
        assert_eq!(s.key_delay_ms, 7);
        assert_eq!(s.countdown_secs, Settings::default().countdown_secs);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let s = Settings::from_toml("key_delay_ms = 999999\ncountdown_secs = 999999\nhotkey = ' '")
            .unwrap();
        assert_eq!(s.key_delay_ms, MAX_KEY_DELAY_MS);
        assert_eq!(s.countdown_secs, MAX_COUNTDOWN_SECS);
        assert_eq!(s.hotkey, DEFAULT_HOTKEY);
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("autotyper-test-{}", std::process::id()));
        let path = dir.join("settings.toml");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "this is = not [valid").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());

        let s = Settings {
            press_enter: true,
            ..Settings::default()
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_text_is_never_part_of_the_settings() {
        let toml = Settings::default().to_toml();
        assert!(!toml.contains("text"));
    }
}
