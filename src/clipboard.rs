//! "Type the clipboard": used by `--clipboard`, the Start menu shortcut and the tray agent.

use zeroize::Zeroizing;

use crate::{
    cli::ClipboardArgs,
    job::{CancelToken, JobEvent, Options, Outcome},
    settings::Settings,
    typing,
};

/// Reads the clipboard as text.
pub fn read() -> Result<Zeroizing<String>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("cannot open the clipboard: {e}"))?;
    let text = clipboard
        .get_text()
        .map_err(|_| "the clipboard has no text".to_string())?;
    if text.is_empty() {
        return Err("the clipboard is empty".into());
    }
    Ok(Zeroizing::new(text))
}

pub fn clear() {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.clear();
    }
}

/// Options for typing the clipboard: saved settings plus command-line overrides.
pub fn options(settings: &Settings, args: &ClipboardArgs) -> Options {
    let mut opts = Options::from_settings(
        settings,
        args.delay_secs.unwrap_or(settings.clipboard_delay_secs),
    );
    if let Some(mode) = args.mode {
        opts.mode = mode;
    }
    if let Some(ms) = args.key_delay_ms {
        opts.key_delay = std::time::Duration::from_millis(u64::from(ms));
    }
    opts.press_enter |= args.enter;
    opts
}

/// Reads the clipboard and types it with `opts`. Blocks until done.
pub fn type_clipboard(
    opts: &Options,
    clear_after: bool,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(JobEvent),
) -> Outcome {
    let text = match read() {
        Ok(text) => text,
        Err(e) => return Outcome::Failed(e),
    };
    let outcome = typing::run(&text, opts, cancel, on_event);
    if clear_after && matches!(outcome, Outcome::Done { .. }) {
        clear();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::job::Mode;

    #[test]
    fn command_line_overrides_settings() {
        let settings = Settings::default();
        let opts = options(
            &settings,
            &ClipboardArgs {
                delay_secs: Some(0),
                mode: Some(Mode::Scancode),
                key_delay_ms: Some(5),
                enter: true,
            },
        );
        assert_eq!(opts.countdown, Duration::ZERO);
        assert_eq!(opts.mode, Mode::Scancode);
        assert_eq!(opts.key_delay, Duration::from_millis(5));
        assert!(opts.press_enter);

        let defaults = options(&settings, &ClipboardArgs::default());
        assert_eq!(
            defaults.countdown,
            Duration::from_secs(settings.clipboard_delay_secs.into())
        );
        assert_eq!(defaults.mode, settings.mode);
    }
}
