//! Where Auto Typer keeps its files.

use std::path::PathBuf;

/// Folder name under the per-user config/data directories.
#[cfg(not(target_os = "linux"))]
pub const APP_DIR: &str = "AutoTyper";
#[cfg(target_os = "linux")]
pub const APP_DIR: &str = "autotyper";

/// `%APPDATA%\AutoTyper`, `~/.config/autotyper`, `~/Library/Application Support/AutoTyper`.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR))
}

pub fn settings_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// Per-user folder for lock files: `%LOCALAPPDATA%\AutoTyper`, `$XDG_RUNTIME_DIR/autotyper`
/// or the cache directory.
pub fn runtime_dir() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::data_local_dir)
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// Name of the local socket / named pipe a running instance listens on.
///
/// `role` is `"gui"` or `"agent"`. The user name is part of the name so that two users on
/// the same machine never talk to each other's instance.
pub fn socket_name(role: &str) -> String {
    format!("autotyper-{}-{role}", user_tag())
}

fn user_tag() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    user.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_are_plain_ascii() {
        let name = socket_name("gui");
        assert!(name.starts_with("autotyper-") && name.ends_with("-gui"));
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        );
    }
}
