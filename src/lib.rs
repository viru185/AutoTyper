//! Auto Typer: types text for you where paste is blocked (RDP lock screens, VM consoles,
//! remote sessions).

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
