//! Command-line parsing (`lexopt`: tiny, no derive macros).

use std::ffi::OsString;

use crate::job::Mode;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the window (no arguments).
    Gui,
    /// Type the clipboard into the focused window, then exit.
    TypeClipboard(ClipboardArgs),
    /// Tray icon + global hotkey.
    Background,
    Install {
        quiet: bool,
    },
    Uninstall {
        quiet: bool,
        purge: bool,
    },
    Help,
    Version,
}

/// Overrides for `--clipboard`; `None` means "use the saved setting".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardArgs {
    pub delay_secs: Option<u32>,
    pub mode: Option<Mode>,
    pub key_delay_ms: Option<u32>,
    pub enter: bool,
}

pub fn help() -> String {
    format!(
        "\
Auto Typer {version}
Types text for you where paste is blocked (RDP lock screens, VM consoles, remote sessions).

Usage: autotyper [OPTIONS]

With no options, opens the Auto Typer window.

Options:
  -c, --clipboard        Type the clipboard into the focused window, then exit
  -d, --delay <SECONDS>  With --clipboard: wait this long first (default: 2)
  -m, --mode <MODE>      With --clipboard: unicode (default) or scancode
  -s, --speed <MS>       With --clipboard: pause between keystrokes (default: 20)
      --enter            With --clipboard: press Enter after the text
      --background       Run in the tray, with the global hotkey
      --install          Install for this user and add Start menu shortcuts
      --uninstall        Remove the installed copy, shortcuts and registry entries
      --purge            With --uninstall: also delete the settings
  -q, --quiet            With --install / --uninstall: no message boxes
  -h, --help             Print help
  -V, --version          Print version
",
        version = env!("CARGO_PKG_VERSION")
    )
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut clipboard = false;
    let mut clip = ClipboardArgs::default();
    let (mut background, mut install, mut uninstall) = (false, false, false);
    let (mut purge, mut quiet) = (false, false);

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Short('c') | Long("clipboard") => clipboard = true,
            Short('d') | Long("delay") => {
                clip.delay_secs = Some(number(&mut parser, "--delay", 120)?);
            }
            Short('s') | Long("speed") => {
                clip.key_delay_ms = Some(number(&mut parser, "--speed", 1000)?);
            }
            Short('m') | Long("mode") => {
                let value = parser.value().map_err(|e| e.to_string())?;
                let value = value.to_string_lossy();
                clip.mode =
                    Some(Mode::parse(&value).ok_or_else(|| {
                        format!("invalid --mode '{value}': use unicode or scancode")
                    })?);
            }
            Long("enter") => clip.enter = true,
            Long("background") => background = true,
            Long("install") => install = true,
            Long("uninstall") => uninstall = true,
            Long("purge") => purge = true,
            Short('q') | Long("quiet") => quiet = true,
            other => return Err(other.unexpected().to_string()),
        }
    }

    let modes = [clipboard, background, install, uninstall];
    if modes.iter().filter(|&&on| on).count() > 1 {
        return Err("use only one of --clipboard, --background, --install, --uninstall".into());
    }
    if !clipboard && clip != ClipboardArgs::default() {
        return Err("--delay, --mode, --speed and --enter only work with --clipboard".into());
    }
    if purge && !uninstall {
        return Err("--purge only works with --uninstall".into());
    }
    if quiet && !(install || uninstall) {
        return Err("--quiet only works with --install or --uninstall".into());
    }

    Ok(if clipboard {
        Command::TypeClipboard(clip)
    } else if background {
        Command::Background
    } else if install {
        Command::Install { quiet }
    } else if uninstall {
        Command::Uninstall { quiet, purge }
    } else {
        Command::Gui
    })
}

fn number(parser: &mut lexopt::Parser, flag: &str, max: u32) -> Result<u32, String> {
    let value = parser.value().map_err(|e| e.to_string())?;
    let n: u32 = value
        .to_string_lossy()
        .parse()
        .map_err(|_| format!("{flag} needs a whole number"))?;
    if n > max {
        return Err(format!("{flag} must be at most {max}"));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_opens_the_window() {
        assert_eq!(p(&[]), Ok(Command::Gui));
    }

    #[test]
    fn clipboard_with_overrides() {
        assert_eq!(
            p(&[
                "--clipboard",
                "-d",
                "5",
                "--mode=scancode",
                "--speed",
                "40",
                "--enter"
            ]),
            Ok(Command::TypeClipboard(ClipboardArgs {
                delay_secs: Some(5),
                mode: Some(Mode::Scancode),
                key_delay_ms: Some(40),
                enter: true,
            }))
        );
        assert_eq!(
            p(&["-c"]),
            Ok(Command::TypeClipboard(ClipboardArgs::default()))
        );
    }

    #[test]
    fn install_and_uninstall_flags() {
        assert_eq!(
            p(&["--install", "-q"]),
            Ok(Command::Install { quiet: true })
        );
        assert_eq!(
            p(&["--uninstall", "--purge"]),
            Ok(Command::Uninstall {
                quiet: false,
                purge: true
            })
        );
        assert_eq!(p(&["--background"]), Ok(Command::Background));
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(p(&["--clipboard", "--help"]), Ok(Command::Help));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn bad_combinations_are_errors() {
        assert!(p(&["--delay", "3"]).is_err());
        assert!(p(&["--clipboard", "--install"]).is_err());
        assert!(p(&["--purge"]).is_err());
        assert!(p(&["--quiet"]).is_err());
        assert!(p(&["-c", "--mode", "morse"]).is_err());
        assert!(p(&["-c", "--delay", "x"]).is_err());
        assert!(p(&["-c", "--delay", "999"]).is_err());
        assert!(p(&["--nope"]).is_err());
        assert!(p(&["stray"]).is_err());
    }

    #[test]
    fn help_lists_every_option() {
        let help = help();
        for flag in [
            "--clipboard",
            "--delay",
            "--mode",
            "--speed",
            "--enter",
            "--background",
            "--install",
            "--uninstall",
            "--purge",
            "--quiet",
            "--help",
            "--version",
        ] {
            assert!(help.contains(flag), "{flag} missing from --help");
        }
    }
}
