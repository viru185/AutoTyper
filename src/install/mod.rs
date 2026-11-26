//! Per-user install / uninstall, so the app shows up in the Start menu (and therefore in
//! Windows Search, PowerToys Command Palette and PowerToys Run) or the Linux app launcher.
//!
//! No admin rights are needed: everything goes into the user's profile.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

use crate::{
    autostart,
    instance::{self, Message, Role},
    paths,
};

/// Where the installed executable lives, if this platform supports installing.
pub fn installed_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    return windows::install_dir().map(|d| d.join("autotyper.exe"));
    #[cfg(target_os = "linux")]
    return linux::installed_exe();
    #[cfg(not(any(windows, target_os = "linux")))]
    return None;
}

/// Is the running executable the installed copy?
pub fn running_installed_copy() -> bool {
    let (Some(installed), Ok(current)) = (installed_exe(), std::env::current_exe()) else {
        return false;
    };
    same_file(&installed, &current)
}

pub fn is_installed() -> bool {
    installed_exe().is_some_and(|p| p.exists())
}

/// Copies the running executable into the per-user install location and creates the
/// launcher entries. Returns a message for the user.
pub fn install() -> Result<String, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    return windows::install(&current);
    #[cfg(target_os = "linux")]
    return linux::install(&current);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = current;
        Err("installing is not supported on this platform yet; see the README".into())
    }
}

/// Removes everything [`install`] created (and the settings with `purge`).
pub fn uninstall(purge: bool) -> Result<String, String> {
    // Ask running instances to exit so their files can be deleted.
    let _ = instance::send(Role::Gui, Message::Quit);
    let _ = instance::send(Role::Agent, Message::Quit);
    std::thread::sleep(std::time::Duration::from_millis(300));
    autostart::disable()?;

    #[cfg(windows)]
    windows::uninstall()?;
    #[cfg(target_os = "linux")]
    linux::uninstall()?;

    if purge {
        if let Some(dir) = paths::config_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = std::fs::remove_dir_all(paths::runtime_dir());
    }
    Ok("Auto Typer was uninstalled.".into())
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
