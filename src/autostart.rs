//! "Start with Windows / at login": launches `autotyper --background`.
//!
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! * Linux: `~/.config/autostart/autotyper.desktop`
//! * macOS: `~/Library/LaunchAgents/io.github.viru185.autotyper.plist`

use std::path::Path;

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE: &str = "AutoTyper";

/// Is autostart registered for the given executable path?
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    return crate::platform::windows::reg_get_string(RUN_KEY, VALUE).is_some();
    #[cfg(not(windows))]
    return entry_path().is_some_and(|p| p.exists());
}

pub fn enable(exe: &Path) -> Result<(), String> {
    #[cfg(windows)]
    return crate::platform::windows::reg_set_string(
        RUN_KEY,
        VALUE,
        &format!("\"{}\" --background", exe.display()),
    );
    #[cfg(not(windows))]
    {
        let path = entry_path().ok_or("no home directory")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, entry_contents(exe)).map_err(|e| e.to_string())
    }
}

pub fn disable() -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::platform::windows::reg_delete_value(RUN_KEY, VALUE);
        Ok(())
    }
    #[cfg(not(windows))]
    match entry_path() {
        Some(p) if p.exists() => std::fs::remove_file(p).map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

#[cfg(target_os = "linux")]
fn entry_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart").join("autotyper.desktop"))
}

#[cfg(target_os = "linux")]
fn entry_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Auto Typer\nExec=\"{}\" --background\n\
         Icon=autotyper\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exe.display()
    )
}

#[cfg(target_os = "macos")]
fn entry_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|d| {
        d.join("Library/LaunchAgents")
            .join("io.github.viru185.autotyper.plist")
    })
}

#[cfg(target_os = "macos")]
fn entry_contents(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.github.viru185.autotyper</string>
  <key>ProgramArguments</key><array><string>{}</string><string>--background</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
        exe.display()
    )
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn entry_path() -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn entry_contents(_exe: &Path) -> String {
    String::new()
}
