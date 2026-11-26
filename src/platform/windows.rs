//! Small Win32 helpers: message boxes, console handling, known folders, registry.

use std::{
    ffi::{OsStr, OsString, c_void},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr::{null, null_mut},
};

use windows_sys::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::{
            Com::CoTaskMemFree,
            Console::{FreeConsole, GetConsoleProcessList, GetConsoleWindow},
            Registry::{
                HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW,
                RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
            },
        },
        UI::{
            Shell::{KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath},
            WindowsAndMessaging::{
                MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW, SW_HIDE,
                ShowWindow,
            },
        },
    },
    core::{GUID, PWSTR},
};

pub use windows_sys::Win32::UI::Shell::{FOLDERID_Programs, FOLDERID_UserProgramFiles};

/// NUL-terminated UTF-16 for Win32 `W` functions.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn message_box(title: &str, text: &str, error: bool) {
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide(title).as_ptr(),
            MB_OK | icon | MB_SETFOREGROUND,
        );
    }
}

/// Gets rid of the console for GUI / background modes.
///
/// The exe is a console program so that terminals wait for it and show `--help`. With the
/// manifest's `consoleAllocationPolicy=detached` (Windows 11 24H2+), starting it from the Start
/// menu creates no console at all. On older Windows a console window was created just for us:
/// hide it at once. With `also_shared`, also let go of a terminal we were started from, so
/// Ctrl+C there does not kill the window.
pub fn detach_console(also_shared: bool) {
    // SAFETY: plain Win32 calls; the list buffer is large enough for the count we pass.
    unsafe {
        let window = GetConsoleWindow();
        if window.is_null() {
            return;
        }
        let mut list = [0u32; 2];
        let ours_alone = GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32) == 1;
        if ours_alone {
            ShowWindow(window, SW_HIDE);
        }
        if ours_alone || also_shared {
            FreeConsole();
        }
    }
}

/// A known folder such as `FOLDERID_Programs` (Start menu → Programs).
///
/// The path is returned even if the folder does not exist yet: on a fresh profile
/// `%LOCALAPPDATA%\Programs` is missing until something is installed there.
pub fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut raw: PWSTR = null_mut();
    // SAFETY: valid out-pointer; the returned buffer is freed with CoTaskMemFree.
    let hr = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY as u32, null_mut(), &mut raw) };
    let path = if hr >= 0 && !raw.is_null() {
        // SAFETY: on success `raw` is a NUL-terminated wide string.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        // SAFETY: `len` units were just checked to be readable.
        let units = unsafe { std::slice::from_raw_parts(raw, len) };
        Some(PathBuf::from(OsString::from_wide(units)))
    } else {
        None
    };
    // SAFETY: freeing null is allowed.
    unsafe { CoTaskMemFree(raw as *const c_void) };
    path
}

// ----------------------------------------------------------------------------- registry (HKCU)

fn check(code: u32, what: &str) -> Result<(), String> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what} failed (error {code})"))
    }
}

/// Sets `HKCU\<key>\<name>` to a string, creating the key if needed.
pub fn reg_set_string(key: &str, name: &str, value: &str) -> Result<(), String> {
    let data = wide(value);
    // Bound to a variable: a temporary created inside the `if` below would be freed before
    // the call reads it.
    let name = (!name.is_empty()).then(|| wide(name));
    // SAFETY: all strings are NUL-terminated and outlive the call; `data` is `cb` bytes long.
    let code = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            name.as_ref().map_or(null(), |n| n.as_ptr()),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    check(code, "writing to the registry")
}

pub fn reg_set_dword(key: &str, name: &str, value: u32) -> Result<(), String> {
    // SAFETY: all strings are NUL-terminated; the data is a u32.
    let code = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            wide(name).as_ptr(),
            REG_DWORD,
            (&value as *const u32).cast(),
            4,
        )
    };
    check(code, "writing to the registry")
}

pub fn reg_get_string(key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `bytes` bytes.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code != ERROR_SUCCESS {
        return None;
    }
    let units = (bytes as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..units.min(buf.len())]))
}

/// Deletes one value; a missing value is not an error.
pub fn reg_delete_value(key: &str, name: &str) {
    // SAFETY: NUL-terminated strings.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(name).as_ptr()) };
}

/// Deletes a key with everything below it; a missing key is not an error.
pub fn reg_delete_tree(key: &str) {
    let key = wide(key);
    // SAFETY: NUL-terminated strings.
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr());
        RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_folders_resolve() {
        let programs = known_folder(&FOLDERID_Programs).expect("Start menu Programs folder");
        assert!(programs.ends_with("Programs"), "{programs:?}");
        assert!(known_folder(&FOLDERID_UserProgramFiles).is_some());
    }

    #[test]
    fn registry_round_trip() {
        let key = format!(r"Software\AutoTyperTest-{}", std::process::id());
        reg_set_string(&key, "name", "välue").unwrap();
        reg_set_dword(&key, "n", 7).unwrap();
        assert_eq!(reg_get_string(&key, "name").as_deref(), Some("välue"));
        reg_delete_value(&key, "name");
        assert_eq!(reg_get_string(&key, "name"), None);
        reg_delete_tree(&key);
        assert_eq!(reg_get_string(&key, "n"), None);
    }
}
