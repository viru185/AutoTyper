//! macOS / Linux keyboard backend, built on `enigo`.
//!
//! * macOS: needs the Accessibility permission (System Settings → Privacy & Security →
//!   Accessibility); `enigo` opens the system prompt the first time.
//! * Linux: X11 (XTEST). Wayland compositors generally block synthetic input.

use enigo::{Direction, Enigo, Key, Keyboard, Mouse, Settings};

use super::Typist;
use crate::{
    job::{AbortReason, Options},
    text::Token,
};

/// How far the mouse may move before typing is aborted.
const MOUSE_SLOP: i32 = 40;

pub struct EnigoTypist {
    enigo: Enigo,
    abort_on_mouse_move: bool,
    start: Option<(i32, i32)>,
}

impl EnigoTypist {
    pub fn new(opts: &Options) -> Result<Self, String> {
        let settings = Settings {
            open_prompt_to_get_permissions: true,
            release_keys_when_dropped: true,
            ..Settings::default()
        };
        let enigo = Enigo::new(&settings).map_err(|e| {
            if cfg!(target_os = "macos") {
                format!(
                    "cannot simulate keyboard input ({e}). Allow Auto Typer in System Settings → \
                     Privacy & Security → Accessibility, then try again"
                )
            } else {
                format!("cannot simulate keyboard input ({e}). An X11 session is required")
            }
        })?;
        Ok(EnigoTypist {
            enigo,
            abort_on_mouse_move: opts.abort_on_mouse_move,
            start: None,
        })
    }
}

impl Typist for EnigoTypist {
    fn esc_pressed(&mut self) -> bool {
        // Reading global key state needs extra permissions here; use the Stop button.
        false
    }

    fn begin(&mut self) -> Result<(), String> {
        self.start = self.enigo.location().ok();
        Ok(())
    }

    fn send(&mut self, token: Token) -> Result<(), String> {
        let result = match token {
            Token::Char(c) => self.enigo.text(c.encode_utf8(&mut [0; 4])),
            Token::Enter => self.enigo.key(Key::Return, Direction::Click),
            Token::Tab => self.enigo.key(Key::Tab, Direction::Click),
        };
        result.map_err(|e| format!("typing failed: {e}"))
    }

    fn abort_reason(&mut self) -> Option<AbortReason> {
        if !self.abort_on_mouse_move {
            return None;
        }
        let (x0, y0) = self.start?;
        let (x, y) = self.enigo.location().ok()?;
        ((x - x0).abs() > MOUSE_SLOP || (y - y0).abs() > MOUSE_SLOP)
            .then_some(AbortReason::MouseMoved)
    }

    fn end(&mut self) {}
}
