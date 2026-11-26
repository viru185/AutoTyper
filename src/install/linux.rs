//! Linux: `~/.local/bin/autotyper`, `.desktop` launcher entries and an icon.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const ICON_PNG: &[u8] = include_bytes!("../../assets/autotyper.png");

pub fn installed_exe() -> Option<PathBuf> {
    dirs::executable_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/bin")))
        .map(|d| d.join("autotyper"))
}

fn applications_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications"))
}

fn icon_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("icons/hicolor/256x256/apps/autotyper.png"))
}

fn entries() -> [(&'static str, &'static str, &'static str, &'static str); 2] {
    [
        (
            "autotyper.desktop",
            "Auto Typer",
            "",
            "Type text where paste is blocked",
        ),
        (
            "autotyper-clipboard.desktop",
            "Auto Typer - Type clipboard",
            " --clipboard",
            "Type the clipboard into the window you were using",
        ),
    ]
}

pub fn install(current: &Path) -> Result<String, String> {
    let exe = installed_exe().ok_or("no home directory")?;
    let apps = applications_dir().ok_or("no data directory")?;
    let icon = icon_file().ok_or("no data directory")?;
    let io = |e: std::io::Error| e.to_string();

    if !super::same_file(current, &exe) {
        fs::create_dir_all(exe.parent().expect("has a parent")).map_err(io)?;
        let tmp = exe.with_extension("tmp");
        fs::copy(current, &tmp).map_err(io)?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(io)?;
        fs::rename(&tmp, &exe).map_err(io)?;
    }
    fs::create_dir_all(icon.parent().expect("has a parent")).map_err(io)?;
    fs::write(&icon, ICON_PNG).map_err(io)?;
    fs::create_dir_all(&apps).map_err(io)?;
    for (file, name, args, comment) in entries() {
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\n\
             Exec=\"{}\"{args}\nIcon=autotyper\nTerminal=false\nCategories=Utility;\n\
             Keywords=type;paste;password;rdp;clipboard;\n",
            exe.display()
        );
        fs::write(apps.join(file), entry).map_err(io)?;
    }
    Ok(format!(
        "Installed to {}. \"Auto Typer\" is now in your application launcher.",
        exe.display()
    ))
}

pub fn uninstall() -> Result<(), String> {
    if let Some(apps) = applications_dir() {
        for (file, ..) in entries() {
            let _ = fs::remove_file(apps.join(file));
        }
    }
    for path in [icon_file(), installed_exe()].into_iter().flatten() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}
