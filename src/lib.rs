//! Auto Typer: types text for you where paste is blocked (RDP lock screens, VM consoles,
//! remote sessions).
//!
//! `main.rs` only collects the arguments and calls [`run`].

#[cfg(feature = "tray")]
pub mod agent;
pub mod app;
pub mod author;
pub mod autostart;
pub mod cli;
pub mod clipboard;
pub mod install;
pub mod instance;
pub mod job;
pub mod paths;
pub mod platform;
pub mod settings;
pub mod text;
pub mod typing;

use std::ffi::OsString;

use cli::Command;
use job::CancelToken;
use settings::Settings;

const TITLE: &str = "Auto Typer";

/// Side effects [`run`] depends on, replaceable in tests.
pub struct Hooks {
    pub author: fn() -> i32,
    pub load_settings: fn() -> Settings,
}

impl Hooks {
    pub fn real() -> Self {
        Hooks {
            author: || author::show(None),
            load_settings: settings::load,
        }
    }
}

/// The command-line entry point. Returns the process exit code.
pub fn run(argv: Vec<OsString>, hooks: &Hooks) -> i32 {
    // Answered before anything else: no parsing, no settings, no console handling.
    if argv.iter().any(|a| a == "--author") {
        return (hooks.author)();
    }

    let command = match cli::parse(argv) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("autotyper: {e}\nRun 'autotyper --help' for usage.");
            return 2;
        }
    };
    match command {
        Command::Help => {
            print!("{}", cli::help());
            0
        }
        Command::Version => {
            println!("autotyper {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Gui => {
            platform::detach_console();
            app::run((hooks.load_settings)())
        }
        Command::TypeClipboard(args) => {
            platform::hide_own_console();
            let settings = (hooks.load_settings)();
            let opts = clipboard::options(&settings, &args);
            let outcome = clipboard::type_clipboard(
                &opts,
                settings.clear_clipboard_after,
                &CancelToken::default(),
                &mut |_| {},
            );
            match outcome {
                job::Outcome::Done { .. } => 0,
                job::Outcome::Cancelled { .. } => {
                    platform::tell(TITLE, &outcome.to_string(), true, true);
                    1
                }
                job::Outcome::Failed(msg) => {
                    platform::tell(TITLE, &format!("Typing failed: {msg}"), true, false);
                    1
                }
            }
        }
        Command::Background => background(),
        Command::Install { quiet } => report(install::install(), quiet),
        Command::Uninstall { quiet, purge } => report(install::uninstall(purge), quiet),
    }
}

#[cfg(feature = "tray")]
fn background() -> i32 {
    platform::detach_console();
    agent::run()
}

#[cfg(not(feature = "tray"))]
fn background() -> i32 {
    eprintln!("autotyper: this build has no tray support (feature \"tray\")");
    2
}

fn report(result: Result<String, String>, quiet: bool) -> i32 {
    match result {
        Ok(msg) => {
            platform::tell(TITLE, &msg, false, quiet);
            0
        }
        Err(e) => {
            platform::tell(TITLE, &e, true, quiet);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    static AUTHOR_SHOWN: AtomicBool = AtomicBool::new(false);

    /// The Rust stand-in for monkeypatching: the author card is stubbed, and reading the
    /// settings fails the test.
    fn hooks() -> Hooks {
        Hooks {
            author: || {
                AUTHOR_SHOWN.store(true, Ordering::SeqCst);
                0
            },
            load_settings: || panic!("--author must not read the settings"),
        }
    }

    #[test]
    fn author_needs_no_settings_and_wins_over_everything() {
        for argv in [
            vec!["--author"],
            vec!["--clipboard", "--author"],
            vec!["--nope", "--author"],
        ] {
            AUTHOR_SHOWN.store(false, Ordering::SeqCst);
            let argv = argv.into_iter().map(OsString::from).collect();
            assert_eq!(run(argv, &hooks()), 0);
            assert!(AUTHOR_SHOWN.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn help_hints_at_author_without_explaining_it() {
        let help = cli::help();
        assert!(help.contains("--author") && help.contains("who built this"));
        assert!(!help.contains(author::NAME));
    }

    #[test]
    fn bad_arguments_exit_with_2() {
        assert_eq!(run(vec!["--nope".into()], &hooks()), 2);
    }
}
