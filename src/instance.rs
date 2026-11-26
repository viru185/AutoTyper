//! One window and one tray agent per user, and a tiny line-based protocol between them.
//!
//! * A lock file (`gui.lock` / `agent.lock`) decides who is the running instance. The OS
//!   releases the lock when the process exits, even after a crash.
//! * The running instance listens on a local socket (a named pipe on Windows). A second
//!   launch sends it one line (`show`, `quit`, `reload`) and exits.

use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    thread,
};

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};

use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Gui,
    Agent,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Gui => "gui",
            Role::Agent => "agent",
        }
    }
}

/// Messages between instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// Bring the window to the front.
    Show,
    /// Exit (used by `--uninstall` and by turning the tray off).
    Quit,
    /// Settings changed on disk: read them again.
    Reload,
}

impl Message {
    fn as_str(self) -> &'static str {
        match self {
            Message::Show => "show",
            Message::Quit => "quit",
            Message::Reload => "reload",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "show" => Some(Message::Show),
            "quit" => Some(Message::Quit),
            "reload" => Some(Message::Reload),
            _ => None,
        }
    }
}

/// Held by the running instance; dropping it releases the role.
pub struct Instance {
    _lock: File,
}

/// Tries to become the running instance for `role`. `Ok(None)`: another one is running.
pub fn claim(role: Role) -> io::Result<Option<Instance>> {
    let dir = paths::runtime_dir();
    fs::create_dir_all(&dir)?;
    let lock = File::create(dir.join(format!("{}.lock", role.name())))?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(Instance { _lock: lock })),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

impl Instance {
    /// Starts a background thread that passes every received message to `on_message`.
    pub fn listen(
        &self,
        role: Role,
        on_message: impl Fn(Message) + Send + 'static,
    ) -> io::Result<()> {
        let path = socket_file(role);
        if let Some(path) = &path {
            // We hold the lock, so an existing socket file is a leftover from a crash.
            let _ = fs::remove_file(path);
        }
        let listener = ListenerOptions::new()
            .name(socket_name(role)?)
            .create_sync()?;
        thread::Builder::new()
            .name(format!("ipc-{}", role.name()))
            .spawn(move || {
                for conn in listener.incoming().filter_map(Result::ok) {
                    let mut conn = BufReader::new(conn);
                    let mut line = String::new();
                    if conn.read_line(&mut line).is_ok() {
                        let _ = conn.get_mut().write_all(b"ok\n");
                        if let Some(message) = Message::parse(&line) {
                            on_message(message);
                        }
                    }
                }
            })?;
        Ok(())
    }
}

/// Sends a message to the running instance. Fails if none is running.
pub fn send(role: Role, message: Message) -> io::Result<()> {
    #[cfg(windows)]
    // Let the running instance bring its window to the front: we were just started by the
    // user, so we are allowed to hand that right on.
    // SAFETY: no preconditions.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    let mut conn = BufReader::new(Stream::connect(socket_name(role)?)?);
    conn.get_mut()
        .write_all(format!("{}\n", message.as_str()).as_bytes())?;
    let mut reply = String::new();
    conn.read_line(&mut reply)?;
    Ok(())
}

/// Is an instance with this role running?
pub fn is_running(role: Role) -> bool {
    match claim(role) {
        Ok(Some(_released_on_drop)) => false,
        Ok(None) => true,
        Err(_) => false,
    }
}

fn socket_name(role: Role) -> io::Result<Name<'static>> {
    let name = paths::socket_name(role.name());
    if GenericNamespaced::is_supported() && cfg!(windows) {
        // Windows: `\\.\pipe\autotyper-<user>-<role>`.
        name.to_ns_name::<GenericNamespaced>()
    } else {
        // Unix: a socket file in the per-user runtime directory (not the abstract namespace,
        // which every user on the machine could connect to).
        socket_file(role)
            .expect("unix has a socket file")
            .to_fs_name::<GenericFilePath>()
    }
}

fn socket_file(role: Role) -> Option<PathBuf> {
    if cfg!(windows) {
        None
    } else {
        Some(paths::runtime_dir().join(format!("{}.sock", role.name())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        for m in [Message::Show, Message::Quit, Message::Reload] {
            assert_eq!(Message::parse(&format!("{}\n", m.as_str())), Some(m));
        }
        assert_eq!(Message::parse("rm -rf"), None);
    }
}
