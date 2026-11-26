//! Windows: `%LOCALAPPDATA%\Programs\AutoTyper`, Start menu shortcuts, "Apps" entry.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::platform::windows::{
    FOLDERID_Programs, FOLDERID_UserProgramFiles, known_folder, reg_delete_tree, reg_set_dword,
    reg_set_string,
};

const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\autotyper.exe";
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AutoTyper";
const SHORTCUT: &str = "Auto Typer.lnk";
const SHORTCUT_CLIPBOARD: &str = "Auto Typer - Type clipboard.lnk";
const REPO: &str = "https://github.com/viru185/AutoTyper";

/// `%LOCALAPPDATA%\Programs\AutoTyper`
pub fn install_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_UserProgramFiles).map(|d| d.join("AutoTyper"))
}

pub fn install(current: &Path) -> Result<String, String> {
    let dir = install_dir().ok_or("cannot find the per-user Programs folder")?;
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let exe = dir.join("autotyper.exe");
    if !super::same_file(current, &exe) {
        copy_exe(current, &exe)?;
    }

    let programs = known_folder(&FOLDERID_Programs).ok_or("cannot find the Start menu folder")?;
    shortcut(
        &programs.join(SHORTCUT),
        &exe,
        "",
        &dir,
        "Type text where paste is blocked",
    )?;
    shortcut(
        &programs.join(SHORTCUT_CLIPBOARD),
        &exe,
        "--clipboard",
        &dir,
        "Type the clipboard into the window you were using",
    )?;
    register(&exe, &dir)?;

    Ok(format!(
        "Installed to {}.\n\nSearch for \"Auto Typer\" in the Start menu, Windows Search or \
         PowerToys Command Palette.",
        dir.display()
    ))
}

pub fn uninstall() -> Result<(), String> {
    if let Some(programs) = known_folder(&FOLDERID_Programs) {
        for name in [SHORTCUT, SHORTCUT_CLIPBOARD] {
            let _ = fs::remove_file(programs.join(name));
        }
    }
    reg_delete_tree(APP_PATHS);
    reg_delete_tree(UNINSTALL);

    let Some(dir) = install_dir() else {
        return Ok(());
    };
    let running_from_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .zip(dir.canonicalize().ok())
        .is_some_and(|(exe, dir)| exe.starts_with(dir));
    if running_from_dir {
        // A running exe cannot delete itself: this schedules it for after we exit.
        self_replace::self_delete_outside_path(&dir)
            .map_err(|e| format!("cannot remove the program file: {e}"))?;
    }
    // Everything else (old copies, the folder if empty); the running exe may linger briefly.
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

/// Copies the exe into place. A running old copy cannot be overwritten, but it can be
/// renamed out of the way.
fn copy_exe(src: &Path, dest: &Path) -> Result<(), String> {
    let old = dest.with_extension("old.exe");
    let _ = fs::remove_file(&old);
    if dest.exists() {
        fs::rename(dest, &old).map_err(|e| format!("cannot replace the installed copy: {e}"))?;
    }
    let tmp = dest.with_extension("tmp");
    fs::copy(src, &tmp)
        .and_then(|_| fs::rename(&tmp, dest))
        .map_err(|e| format!("cannot copy the program: {e}"))?;
    // Succeeds unless the old copy is still running; then it is removed next time.
    let _ = fs::remove_file(&old);
    Ok(())
}

fn shortcut(
    lnk: &Path,
    target: &Path,
    args: &str,
    dir: &Path,
    description: &str,
) -> Result<(), String> {
    let err = |e: mslnk::MSLinkError| format!("cannot create {}: {e:?}", lnk.display());
    let mut link = mslnk::ShellLink::new(target).map_err(err)?;
    if !args.is_empty() {
        link.set_arguments(Some(args.into()));
    }
    link.set_working_dir(Some(dir.display().to_string()));
    link.set_name(Some(description.into()));
    link.set_icon_location(Some(target.display().to_string()));
    link.create_lnk(lnk).map_err(err)
}

/// "App Paths" (for Win+R `autotyper`) and the "Installed apps" entry.
fn register(exe: &Path, dir: &Path) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    let size_kb = fs::metadata(exe).map_or(0, |m| (m.len() / 1024) as u32);

    reg_set_string(APP_PATHS, "", &exe_s)?;
    reg_set_string(APP_PATHS, "Path", &dir.display().to_string())?;

    reg_set_string(UNINSTALL, "DisplayName", "Auto Typer")?;
    reg_set_string(UNINSTALL, "DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    reg_set_string(UNINSTALL, "Publisher", "viru185")?;
    reg_set_string(UNINSTALL, "DisplayIcon", &format!("{exe_s},0"))?;
    reg_set_string(UNINSTALL, "InstallLocation", &dir.display().to_string())?;
    reg_set_string(
        UNINSTALL,
        "UninstallString",
        &format!("\"{exe_s}\" --uninstall"),
    )?;
    reg_set_string(
        UNINSTALL,
        "QuietUninstallString",
        &format!("\"{exe_s}\" --uninstall --quiet"),
    )?;
    reg_set_string(UNINSTALL, "URLInfoAbout", REPO)?;
    reg_set_string(UNINSTALL, "HelpLink", &format!("{REPO}/issues"))?;
    reg_set_dword(UNINSTALL, "NoModify", 1)?;
    reg_set_dword(UNINSTALL, "NoRepair", 1)?;
    reg_set_dword(UNINSTALL, "EstimatedSize", size_kb)?;
    Ok(())
}
