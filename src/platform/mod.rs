//! Thin platform layer: messages to the user and console handling.

#[cfg(windows)]
pub mod windows;

/// Shows a message box on Windows (the app usually has no console there), stderr elsewhere.
pub fn notify(title: &str, text: &str, error: bool) {
    #[cfg(windows)]
    windows::message_box(title, text, error);
    #[cfg(not(windows))]
    {
        let _ = error;
        eprintln!("{title}: {text}");
    }
}

/// Reports a result: printed when there is a console (started from a terminal), otherwise a
/// message box unless `quiet`.
pub fn tell(title: &str, text: &str, error: bool, quiet: bool) {
    if has_console() {
        if error {
            eprintln!("autotyper: {text}");
        } else {
            println!("{text}");
        }
    } else if !quiet {
        notify(title, text, error);
    }
}

/// Is this process attached to a console / terminal?
pub fn has_console() -> bool {
    #[cfg(windows)]
    // SAFETY: no preconditions.
    return !unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null();
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    }
}

/// Lets go of the console before showing a window (see [`windows::detach_console`]).
pub fn detach_console() {
    #[cfg(windows)]
    windows::detach_console(true);
}

/// Hides a console window that Windows created just for us (pre-24H2 Windows, started from
/// the Start menu), but keeps a terminal we were started from, for messages.
pub fn hide_own_console() {
    #[cfg(windows)]
    windows::detach_console(false);
}
