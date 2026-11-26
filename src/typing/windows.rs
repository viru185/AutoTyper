//! Windows keyboard backend: `SendInput` with Unicode packets or scan codes.
//!
//! Scan codes (not virtual-key codes) are what Remote Desktop, VMs and games forward, so key
//! presses are always sent with `KEYEVENTF_SCANCODE`.

use std::{
    collections::HashMap,
    mem::size_of,
    ptr::null_mut,
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, HWND, POINT},
    Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel,
    },
    System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD,
            KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
            KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC_EX, MapVirtualKeyExW, SendInput, ToUnicodeEx,
            VK_CAPITAL, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RMENU,
            VK_RWIN, VK_SHIFT, VkKeyScanExW,
        },
        WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId},
    },
};

use super::{
    Typist,
    plan::{self, KeyPress, Layout, Stroke},
};
use crate::{
    job::{AbortReason, Mode, Options},
    text::Token,
};

const SCAN_LSHIFT: u16 = 0x2A;
const SCAN_LCTRL: u16 = 0x1D;
/// Right Alt (AltGr) is `E0 38`.
const SCAN_RALT: u16 = 0x38;
const SCAN_CAPSLOCK: u16 = 0x3A;
/// How far the mouse may move before typing is aborted.
const MOUSE_SLOP: i32 = 40;
const MODIFIER_WAIT: Duration = Duration::from_secs(3);

/// Caps Lock was switched off by us and must be switched back on. A static so the panic
/// hook can restore it too.
static CAPS_TOGGLED: AtomicBool = AtomicBool::new(false);

pub struct WinTypist {
    mode: Mode,
    abort_on_mouse_move: bool,
    abort_on_focus_change: bool,
    target: HWND,
    layout: WinLayout,
    start_cursor: POINT,
    cache: HashMap<char, Stroke>,
}

impl WinTypist {
    pub fn new(opts: &Options) -> Self {
        install_panic_guard();
        WinTypist {
            mode: opts.mode,
            abort_on_mouse_move: opts.abort_on_mouse_move,
            abort_on_focus_change: opts.abort_on_focus_change,
            target: null_mut(),
            layout: WinLayout { hkl: null_mut() },
            start_cursor: POINT { x: 0, y: 0 },
            cache: HashMap::new(),
        }
    }
}

impl Typist for WinTypist {
    fn esc_pressed(&mut self) -> bool {
        key_down(VK_ESCAPE)
    }

    fn begin(&mut self) -> Result<(), String> {
        // SAFETY: plain Win32 calls with valid out-pointers.
        let (target, pid, tid) = unsafe {
            let target = GetForegroundWindow();
            let mut pid = 0u32;
            let tid = GetWindowThreadProcessId(target, &mut pid);
            (target, pid, tid)
        };
        if target.is_null() {
            return Err("no window has the keyboard focus".into());
        }
        // SAFETY: no preconditions.
        if pid == unsafe { GetCurrentProcessId() } {
            return Err(
                "Auto Typer itself has the focus. Click into the window you want to type into \
                 before the countdown ends."
                    .into(),
            );
        }
        if let (Some(theirs), Some(ours)) = (process_integrity(pid), own_integrity())
            && theirs > ours
        {
            return Err(
                "the focused window runs as administrator, and Windows does not let a normal \
                 app type into it. Start Auto Typer as administrator for this window."
                    .into(),
            );
        }

        wait_for_modifiers_released();

        self.target = target;
        // SAFETY: plain Win32 calls.
        self.layout = WinLayout {
            hkl: unsafe { GetKeyboardLayout(tid) },
        };
        // SAFETY: valid out-pointer.
        unsafe { GetCursorPos(&mut self.start_cursor) };

        // Caps Lock would turn scan-code letters into the wrong case. Unicode is unaffected.
        if self.mode == Mode::Scancode && caps_lock_on() {
            tap(SCAN_CAPSLOCK)?;
            CAPS_TOGGLED.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn send(&mut self, token: Token) -> Result<(), String> {
        let stroke = match token {
            Token::Char(c) => self
                .cache
                .entry(c)
                .or_insert_with(|| plan::plan(token, self.mode, &self.layout))
                .clone(),
            other => plan::plan(other, self.mode, &self.layout),
        };
        match stroke {
            Stroke::Key(key) => press(key),
            Stroke::Unicode(units) => send_unicode(&units),
        }
    }

    fn abort_reason(&mut self) -> Option<AbortReason> {
        if key_down(VK_ESCAPE) {
            return Some(AbortReason::EscKey);
        }
        if self.abort_on_mouse_move {
            let mut p = POINT { x: 0, y: 0 };
            // SAFETY: valid out-pointer.
            if unsafe { GetCursorPos(&mut p) } != 0
                && ((p.x - self.start_cursor.x).abs() > MOUSE_SLOP
                    || (p.y - self.start_cursor.y).abs() > MOUSE_SLOP)
            {
                return Some(AbortReason::MouseMoved);
            }
        }
        // SAFETY: no preconditions.
        if self.abort_on_focus_change && unsafe { GetForegroundWindow() } != self.target {
            return Some(AbortReason::FocusChanged);
        }
        None
    }

    fn end(&mut self) {
        restore_caps_lock();
    }
}

impl Drop for WinTypist {
    fn drop(&mut self) {
        restore_caps_lock();
    }
}

// ----------------------------------------------------------------------------- layout

struct WinLayout {
    hkl: HKL,
}

impl Layout for WinLayout {
    fn vk_for(&self, c: char) -> Option<(u8, u8)> {
        // SAFETY: no pointer arguments.
        let r = unsafe { VkKeyScanExW(u32::from(c) as u16, self.hkl) };
        if r == -1 {
            return None;
        }
        let r = r as u16;
        Some(((r & 0xFF) as u8, (r >> 8) as u8))
    }

    fn scan_for(&self, vk: u8) -> u16 {
        // SAFETY: no pointer arguments.
        unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC_EX, self.hkl) as u16 }
    }

    fn char_for(&self, vk: u8, scan: u16, shift: bool, altgr: bool) -> Option<char> {
        let mut state = [0u8; 256];
        if shift {
            state[usize::from(VK_SHIFT)] = 0x80;
            state[usize::from(VK_LSHIFT)] = 0x80;
        }
        if altgr {
            state[usize::from(VK_CONTROL)] = 0x80;
            state[usize::from(VK_LCONTROL)] = 0x80;
            state[usize::from(VK_MENU)] = 0x80;
            state[usize::from(VK_RMENU)] = 0x80;
        }
        let mut buf = [0u16; 8];
        // Flag 0x4: do not change the keyboard state (no dead-key side effects).
        // SAFETY: `state` has 256 entries and `buf` the length we pass.
        let n = unsafe {
            ToUnicodeEx(
                u32::from(vk),
                u32::from(scan & 0xFF),
                state.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as i32,
                0x4,
                self.hkl,
            )
        };
        if n != 1 {
            return None;
        }
        char::from_u32(u32::from(buf[0]))
    }
}

// ----------------------------------------------------------------------------- input

fn key_input(scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn scan_input(scan: u16, extended: bool, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    key_input(scan, flags)
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    // SAFETY: `inputs` is a valid slice of INPUT structs.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        // SAFETY: no preconditions.
        let code = unsafe { GetLastError() };
        Err(format!(
            "Windows accepted only {sent} of {} key events (error {code}); another program may \
             be blocking keyboard input",
            inputs.len()
        ))
    }
}

/// Presses a key with its modifiers: modifiers down, key down, key up, modifiers up.
fn press(key: KeyPress) -> Result<(), String> {
    let mut down = Vec::with_capacity(4);
    if key.shift {
        down.push(scan_input(SCAN_LSHIFT, false, false));
    }
    if key.altgr {
        down.push(scan_input(SCAN_LCTRL, false, false));
        down.push(scan_input(SCAN_RALT, true, false));
    }
    down.push(scan_input(key.scan, key.extended, false));

    let mut up = Vec::with_capacity(4);
    up.push(scan_input(key.scan, key.extended, true));
    if key.altgr {
        up.push(scan_input(SCAN_RALT, true, true));
        up.push(scan_input(SCAN_LCTRL, false, true));
    }
    if key.shift {
        up.push(scan_input(SCAN_LSHIFT, false, true));
    }

    let result = send(&down);
    // Always try to release, even if pressing failed half-way.
    let released = send(&up);
    result.and(released)
}

fn tap(scan: u16) -> Result<(), String> {
    send(&[
        scan_input(scan, false, false),
        scan_input(scan, false, true),
    ])
}

fn send_unicode(units: &[u16]) -> Result<(), String> {
    let mut inputs = Vec::with_capacity(units.len() * 2);
    inputs.extend(units.iter().map(|&u| key_input(u, KEYEVENTF_UNICODE)));
    inputs.extend(
        units
            .iter()
            .map(|&u| key_input(u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)),
    );
    send(&inputs)
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no preconditions.
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000) != 0
}

fn caps_lock_on() -> bool {
    // SAFETY: no preconditions.
    unsafe { GetKeyState(i32::from(VK_CAPITAL)) & 1 != 0 }
}

/// Waits until the user lets go of Shift/Ctrl/Alt/Win (e.g. after a hotkey), or 3 s.
fn wait_for_modifiers_released() {
    let deadline = Instant::now() + MODIFIER_WAIT;
    let held = || {
        [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
            .into_iter()
            .any(key_down)
    };
    while held() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
}

fn restore_caps_lock() {
    if CAPS_TOGGLED.swap(false, Ordering::SeqCst) {
        let _ = tap(SCAN_CAPSLOCK);
    }
}

/// With `panic = "abort"` destructors do not run, so release everything from the panic
/// hook: no modifier must stay stuck down and Caps Lock must come back.
fn install_panic_guard() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = send(&[
                scan_input(SCAN_LSHIFT, false, true),
                scan_input(SCAN_LCTRL, false, true),
                scan_input(SCAN_RALT, true, true),
            ]);
            restore_caps_lock();
            previous(info);
        }));
    });
}

// ----------------------------------------------------------------------------- integrity

/// Mandatory integrity level (RID) of a process: 0x2000 medium, 0x3000 high (elevated).
fn process_integrity(pid: u32) -> Option<u32> {
    // SAFETY: the handle is closed below; failure returns null.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        // Typically a protected process or another user's: we cannot tell, so do not block.
        return None;
    }
    let level = token_integrity(process);
    // SAFETY: `process` is a valid handle we own.
    unsafe { CloseHandle(process) };
    level
}

fn own_integrity() -> Option<u32> {
    // SAFETY: the pseudo handle needs no closing.
    token_integrity(unsafe { GetCurrentProcess() })
}

fn token_integrity(process: HANDLE) -> Option<u32> {
    let mut token: HANDLE = null_mut();
    // SAFETY: valid out-pointer; the token is closed below.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut len = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(token, TokenIntegrityLevel, null_mut(), 0, &mut len) };
    // u64 storage keeps the buffer aligned for TOKEN_MANDATORY_LABEL.
    let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: `buf` holds at least `len` bytes.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    let level = (ok != 0).then(|| {
        // SAFETY: on success the buffer starts with a TOKEN_MANDATORY_LABEL whose SID points
        // into the same buffer; the last sub-authority is the integrity RID.
        unsafe {
            let label = &*(buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
            let sid = label.Label.Sid;
            let count = u32::from(*GetSidSubAuthorityCount(sid));
            *GetSidSubAuthority(sid, count.saturating_sub(1))
        }
    });
    // SAFETY: `token` is a valid handle we own.
    unsafe { CloseHandle(token) };
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_integrity_level_is_readable() {
        let level = own_integrity().expect("read our integrity level");
        assert!(level >= 0x1000, "unexpected level {level:#x}");
    }

    #[test]
    fn the_us_or_local_layout_maps_letters_with_shift() {
        // SAFETY: plain call.
        let layout = WinLayout {
            hkl: unsafe { GetKeyboardLayout(0) },
        };
        // Every Latin layout types 'a' without modifiers and 'A' with Shift.
        let (vk_a, state_a) = layout.vk_for('a').expect("'a' on the layout");
        let (vk_cap, state_cap) = layout.vk_for('A').expect("'A' on the layout");
        assert_eq!(vk_a, vk_cap);
        assert_eq!((state_a, state_cap), (0, 1));
        let scan = layout.scan_for(vk_a);
        assert_ne!(scan, 0);
        assert_eq!(layout.char_for(vk_a, scan, true, false), Some('A'));
    }
}
