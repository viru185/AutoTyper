//! A typing job: what to type, how, and the events it reports while running.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// How characters are sent to the target window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Unicode packets: independent of keyboard layouts. Works over RDP, including the
    /// Windows lock screen. The default.
    #[default]
    Unicode,
    /// Physical key presses (scan codes + Shift/AltGr) for the current keyboard layout.
    /// For VM consoles, KVMs and web gateways that ignore Unicode input. The *remote*
    /// keyboard layout must match the local one.
    Scancode,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Unicode, Mode::Scancode];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Unicode => "Unicode (recommended)",
            Mode::Scancode => "Key presses (scan codes)",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "unicode" | "u" => Some(Mode::Unicode),
            "scancode" | "scan" | "keys" | "s" => Some(Mode::Scancode),
            _ => None,
        }
    }
}

/// Everything the typing engine needs to know, apart from the text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub countdown: Duration,
    pub key_delay: Duration,
    pub mode: Mode,
    pub trim_trailing_newline: bool,
    pub press_enter: bool,
    pub abort_on_mouse_move: bool,
    pub abort_on_focus_change: bool,
}

impl Options {
    /// Options from the saved settings, with an explicit countdown.
    pub fn from_settings(s: &Settings, countdown_secs: u32) -> Self {
        Options {
            countdown: Duration::from_secs(u64::from(countdown_secs)),
            key_delay: Duration::from_millis(u64::from(s.key_delay_ms)),
            mode: s.mode,
            trim_trailing_newline: s.trim_trailing_newline,
            press_enter: s.press_enter,
            abort_on_mouse_move: s.abort_on_mouse_move,
            abort_on_focus_change: s.abort_on_focus_change,
        }
    }
}

/// Shared flag to stop a running job from another thread.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why typing stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// Stop button, window closed, or another instance asked us to stop.
    User,
    EscKey,
    MouseMoved,
    FocusChanged,
}

impl fmt::Display for AbortReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AbortReason::User => "stopped",
            AbortReason::EscKey => "Esc pressed",
            AbortReason::MouseMoved => "mouse moved",
            AbortReason::FocusChanged => "another window got the focus",
        })
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done { typed: usize },
    Cancelled { reason: AbortReason, typed: usize },
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Done { .. } => f.write_str("Done"),
            Outcome::Cancelled { reason, typed: 0 } => write!(f, "Cancelled: {reason}"),
            Outcome::Cancelled { reason, typed } => {
                write!(f, "Cancelled after {typed} keys: {reason}")
            }
            Outcome::Failed(msg) => write!(f, "Failed: {msg}"),
        }
    }
}

/// Progress reported by a running job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// Seconds left before typing starts.
    Countdown(u32),
    /// Typing started; `total` keystrokes to go.
    Started {
        total: usize,
    },
    Progress {
        done: usize,
        total: usize,
    },
    Finished(Outcome),
}
