//! Auto Typer: types text for you where paste is blocked (RDP lock screens, VM consoles,
//! remote sessions).
//!
//! `main.rs` only collects the arguments and calls [`run`].

#[cfg(feature = "tray")]
pub mod agent;
pub mod app;
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
    pub load_settings: fn() -> Settings,
}

impl Hooks {
    pub fn real() -> Self {
        Hooks {
            load_settings: settings::load,
        }
    }
}

/// The command-line entry point. Returns the process exit code.
pub fn run(argv: Vec<OsString>, hooks: &Hooks) -> i32 {
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
    use super::*;

    fn hooks() -> Hooks {
        Hooks {
            load_settings: || panic!("this test must not read the settings"),
        }
    }

    #[test]
    fn bad_arguments_exit_with_2() {
        assert_eq!(run(vec!["--nope".into()], &hooks()), 2);
    }
}
