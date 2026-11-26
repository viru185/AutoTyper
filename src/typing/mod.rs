//! The typing engine: countdown, then one keystroke at a time with abort checks.
//!
//! The platform-specific part is the [`Typist`] trait: Windows has its own `SendInput`
//! backend; macOS and Linux use `enigo`.

pub mod plan;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod enigo_backend;
#[cfg(windows)]
pub mod windows;

use std::time::Duration;

use crate::{
    job::{AbortReason, CancelToken, JobEvent, Options, Outcome},
    text::{self, Token},
};

/// Sends keystrokes to whatever window has the focus.
pub trait Typist {
    /// Is Esc held down right now? Used to cancel during the countdown.
    fn esc_pressed(&mut self) -> bool;
    /// Called once, when the countdown ends: look at the focused window and get ready.
    fn begin(&mut self) -> Result<(), String>;
    fn send(&mut self, token: Token) -> Result<(), String>;
    /// Checked before every keystroke.
    fn abort_reason(&mut self) -> Option<AbortReason>;
    /// Always called at the end, also after errors: restore Caps Lock etc.
    fn end(&mut self);
}

/// Creates the keyboard backend for this platform.
pub fn new_typist(opts: &Options) -> Result<Box<dyn Typist>, String> {
    #[cfg(windows)]
    return Ok(Box::new(windows::WinTypist::new(opts)));
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return Ok(Box::new(enigo_backend::EnigoTypist::new(opts)?));
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = opts;
        Err("typing is not supported on this platform".into())
    }
}

/// Runs a whole job on the current thread (call it from a worker thread).
pub fn run(
    text: &str,
    opts: &Options,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(JobEvent),
) -> Outcome {
    let outcome = match typing_lock() {
        Ok(_lock) => match new_typist(opts) {
            Ok(mut typist) => run_with(typist.as_mut(), text, opts, cancel, on_event, &|d| {
                std::thread::sleep(d)
            }),
            Err(e) => Outcome::Failed(e),
        },
        Err(e) => Outcome::Failed(e),
    };
    on_event(JobEvent::Finished(outcome.clone()));
    outcome
}

/// Two jobs typing at once (window + tray + Start menu shortcut) would interleave their
/// keystrokes. The OS releases the lock when the file handle is dropped.
fn typing_lock() -> Result<std::fs::File, String> {
    let dir = crate::paths::runtime_dir();
    let lock = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::File::create(dir.join("typing.lock")))
        .map_err(|e| format!("cannot create the typing lock: {e}"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("another Auto Typer is already typing".into())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(format!("typing lock: {e}")),
    }
}

const TICK: Duration = Duration::from_millis(100);

/// The engine itself, independent of the platform (and of real time, for tests).
pub fn run_with(
    typist: &mut dyn Typist,
    text: &str,
    opts: &Options,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(JobEvent),
    sleep: &dyn Fn(Duration),
) -> Outcome {
    let mut tokens = text::tokenize(text, opts.trim_trailing_newline);
    if opts.press_enter {
        tokens.push(Token::Enter);
    }
    if tokens.is_empty() {
        return Outcome::Failed("there is nothing to type".into());
    }

    // Countdown: time to click into the target window.
    let mut left = opts.countdown;
    let mut shown = None;
    while !left.is_zero() {
        let secs = left.as_millis().div_ceil(1000) as u32;
        if shown != Some(secs) {
            on_event(JobEvent::Countdown(secs));
            shown = Some(secs);
        }
        if cancel.is_cancelled() {
            return cancelled(AbortReason::User, 0);
        }
        if typist.esc_pressed() {
            return cancelled(AbortReason::EscKey, 0);
        }
        let step = TICK.min(left);
        sleep(step);
        left -= step;
    }
    if cancel.is_cancelled() {
        return cancelled(AbortReason::User, 0);
    }

    if let Err(e) = typist.begin() {
        typist.end();
        return Outcome::Failed(e);
    }
    let total = tokens.len();
    on_event(JobEvent::Started { total });

    let mut outcome = Outcome::Done { typed: total };
    for (i, token) in tokens.into_iter().enumerate() {
        if i > 0 && !opts.key_delay.is_zero() {
            sleep(opts.key_delay);
        }
        if cancel.is_cancelled() {
            outcome = cancelled(AbortReason::User, i);
            break;
        }
        if let Some(reason) = typist.abort_reason() {
            outcome = cancelled(reason, i);
            break;
        }
        if let Err(e) = typist.send(token) {
            outcome = Outcome::Failed(e);
            break;
        }
        on_event(JobEvent::Progress { done: i + 1, total });
    }
    typist.end();
    outcome
}

fn cancelled(reason: AbortReason, typed: usize) -> Outcome {
    Outcome::Cancelled { reason, typed }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Duration};

    use super::*;
    use crate::job::Mode;

    #[derive(Default)]
    struct Fake {
        sent: Vec<Token>,
        began: bool,
        ended: bool,
        abort_after: Option<(usize, AbortReason)>,
        fail_begin: bool,
    }

    impl Typist for Fake {
        fn esc_pressed(&mut self) -> bool {
            false
        }
        fn begin(&mut self) -> Result<(), String> {
            self.began = true;
            if self.fail_begin {
                Err("nope".into())
            } else {
                Ok(())
            }
        }
        fn send(&mut self, token: Token) -> Result<(), String> {
            self.sent.push(token);
            Ok(())
        }
        fn abort_reason(&mut self) -> Option<AbortReason> {
            match self.abort_after {
                Some((n, reason)) if self.sent.len() >= n => Some(reason),
                _ => None,
            }
        }
        fn end(&mut self) {
            self.ended = true;
        }
    }

    fn opts() -> Options {
        Options {
            countdown: Duration::from_millis(2500),
            key_delay: Duration::from_millis(10),
            mode: Mode::Unicode,
            trim_trailing_newline: true,
            press_enter: false,
            abort_on_mouse_move: true,
            abort_on_focus_change: true,
        }
    }

    fn run_fake(
        fake: &mut Fake,
        text: &str,
        opts: &Options,
        cancel: &CancelToken,
    ) -> (Outcome, Vec<JobEvent>, Duration) {
        let mut events = vec![];
        let slept = Cell::new(Duration::ZERO);
        let outcome = run_with(fake, text, opts, cancel, &mut |e| events.push(e), &|d| {
            slept.set(slept.get() + d)
        });
        (outcome, events, slept.get())
    }

    #[test]
    fn types_everything_after_the_countdown() {
        let mut fake = Fake::default();
        let (outcome, events, slept) =
            run_fake(&mut fake, "ab\n", &opts(), &CancelToken::default());
        assert_eq!(outcome, Outcome::Done { typed: 2 });
        assert_eq!(fake.sent, vec![Token::Char('a'), Token::Char('b')]);
        assert!(fake.began && fake.ended);
        assert_eq!(
            events,
            vec![
                JobEvent::Countdown(3),
                JobEvent::Countdown(2),
                JobEvent::Countdown(1),
                JobEvent::Started { total: 2 },
                JobEvent::Progress { done: 1, total: 2 },
                JobEvent::Progress { done: 2, total: 2 },
            ]
        );
        // 2.5 s countdown + one 10 ms gap between the two keys.
        assert_eq!(slept, Duration::from_millis(2510));
    }

    #[test]
    fn press_enter_adds_a_final_enter() {
        let mut fake = Fake::default();
        let o = Options {
            press_enter: true,
            ..opts()
        };
        run_fake(&mut fake, "x", &o, &CancelToken::default());
        assert_eq!(fake.sent, vec![Token::Char('x'), Token::Enter]);
    }

    #[test]
    fn abort_checks_stop_typing_and_still_clean_up() {
        let mut fake = Fake {
            abort_after: Some((2, AbortReason::MouseMoved)),
            ..Fake::default()
        };
        let (outcome, ..) = run_fake(&mut fake, "abcdef", &opts(), &CancelToken::default());
        assert_eq!(
            outcome,
            Outcome::Cancelled {
                reason: AbortReason::MouseMoved,
                typed: 2
            }
        );
        assert_eq!(fake.sent.len(), 2);
        assert!(fake.ended);
    }

    #[test]
    fn cancelling_during_the_countdown_types_nothing() {
        let mut fake = Fake::default();
        let cancel = CancelToken::default();
        cancel.cancel();
        let (outcome, ..) = run_fake(&mut fake, "abc", &opts(), &cancel);
        assert_eq!(
            outcome,
            Outcome::Cancelled {
                reason: AbortReason::User,
                typed: 0
            }
        );
        assert!(fake.sent.is_empty() && !fake.began);
    }

    #[test]
    fn begin_errors_are_reported() {
        let mut fake = Fake {
            fail_begin: true,
            ..Fake::default()
        };
        let (outcome, ..) = run_fake(&mut fake, "abc", &opts(), &CancelToken::default());
        assert_eq!(outcome, Outcome::Failed("nope".into()));
        assert!(fake.sent.is_empty() && fake.ended);
    }

    #[test]
    fn empty_text_is_an_error() {
        let mut fake = Fake::default();
        let (outcome, ..) = run_fake(&mut fake, "\n", &opts(), &CancelToken::default());
        assert!(matches!(outcome, Outcome::Failed(_)));
    }
}
