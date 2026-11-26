//! `--background`: a tray icon and a global hotkey that types the clipboard.
//!
//! Runs a bare winit event loop with no window (no OpenGL context, a few MB of memory).
//! Tray, menu, hotkey and IPC events arrive on other threads and are forwarded into the loop
//! as [`UserEvent`]s. "Open Auto Typer" starts (or shows) the normal window process.

use std::{
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};
use winit::{
    application::ApplicationHandler,
    event::{StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::WindowId,
};

use crate::{
    cli::ClipboardArgs,
    clipboard,
    instance::{self, Instance, Message, Role},
    job::{CancelToken, Outcome},
    platform,
    settings::{self, Settings},
};

const TITLE: &str = "Auto Typer";

#[derive(Debug)]
enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    HotKey(GlobalHotKeyEvent),
    Ipc(Message),
    JobDone(Outcome),
}

type Proxy = Arc<Mutex<EventLoopProxy<UserEvent>>>;

fn forward(proxy: &Proxy, event: UserEvent) {
    if let Ok(proxy) = proxy.lock() {
        let _ = proxy.send_event(event);
    }
}

/// Runs the tray agent until "Quit". Returns the process exit code.
pub fn run() -> i32 {
    let instance = match instance::claim(Role::Agent) {
        Ok(Some(instance)) => instance,
        // Already running: nothing to do.
        Ok(None) => return 0,
        Err(e) => {
            platform::notify(TITLE, &format!("Cannot start the tray icon: {e}"), true);
            return 1;
        }
    };

    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // No Dock icon: this is a menu-bar-only process.
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            platform::notify(TITLE, &format!("Cannot start the tray icon: {e}"), true);
            return 1;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy: Proxy = Arc::new(Mutex::new(event_loop.create_proxy()));

    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| forward(&p, UserEvent::Tray(e))));
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(Arc::new(move |e| forward(&p, UserEvent::Menu(e)))));
    let p = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| forward(&p, UserEvent::HotKey(e))));
    let p = proxy.clone();
    if let Err(e) = instance.listen(Role::Agent, move |m| forward(&p, UserEvent::Ipc(m))) {
        eprintln!("autotyper: IPC unavailable: {e}");
    }

    let mut agent = Agent {
        proxy,
        settings: settings::load(),
        tray: None,
        type_item: MenuItem::new("Type clipboard", true, None),
        open_id: MenuId::new("open"),
        quit_id: MenuId::new("quit"),
        hotkeys: None,
        hotkey: None,
        busy: Arc::new(AtomicBool::new(false)),
        _instance: instance,
    };
    match event_loop.run_app(&mut agent) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("autotyper: {e}");
            1
        }
    }
}

struct Agent {
    proxy: Proxy,
    settings: Settings,
    tray: Option<TrayIcon>,
    type_item: MenuItem,
    open_id: MenuId,
    quit_id: MenuId,
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    busy: Arc<AtomicBool>,
    _instance: Instance,
}

impl Agent {
    /// Creates the tray icon. Must run once the event loop is running (macOS requirement).
    fn init(&mut self) {
        let menu = Menu::new();
        let open = MenuItem::with_id(self.open_id.clone(), "Open Auto Typer", true, None);
        let quit = MenuItem::with_id(self.quit_id.clone(), "Quit", true, None);
        let _ = menu.append_items(&[
            &self.type_item,
            &open,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        let icon = Icon::from_rgba(
            include_bytes!(concat!(env!("OUT_DIR"), "/icon32.rgba")).to_vec(),
            32,
            32,
        )
        .expect("the embedded icon is valid");
        match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(TITLE)
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
        {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => platform::notify(TITLE, &format!("Cannot create the tray icon: {e}"), true),
        }
        self.register_hotkey();
    }

    fn register_hotkey(&mut self) {
        if let (Some(manager), Some(old)) = (&self.hotkeys, self.hotkey.take()) {
            let _ = manager.unregister(old);
        }
        let mut label = "Type clipboard".to_string();
        if self.settings.hotkey_enabled {
            if self.hotkeys.is_none() {
                self.hotkeys = GlobalHotKeyManager::new().ok();
            }
            match (&self.hotkeys, self.settings.hotkey.parse::<HotKey>()) {
                (Some(manager), Ok(hotkey)) => match manager.register(hotkey) {
                    Ok(()) => {
                        self.hotkey = Some(hotkey);
                        label = format!("Type clipboard\t{}", self.settings.hotkey);
                    }
                    Err(e) => platform::notify(
                        TITLE,
                        &format!(
                            "The hotkey {} is not available ({e}).",
                            self.settings.hotkey
                        ),
                        true,
                    ),
                },
                (_, Err(e)) => platform::notify(
                    TITLE,
                    &format!("Invalid hotkey \"{}\": {e}", self.settings.hotkey),
                    true,
                ),
                (None, _) => {}
            }
        }
        self.type_item.set_text(label);
    }

    fn set_tooltip(&self, text: &str) {
        if let Some(tray) = &self.tray {
            let _ = tray.set_tooltip(Some(text));
        }
    }

    /// Types the clipboard on a worker thread. `delay_secs`: time to click into the target.
    fn type_clipboard(&mut self, delay_secs: u32) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_tooltip(&format!("{TITLE} - typing..."));
        let opts = clipboard::options(
            &self.settings,
            &ClipboardArgs {
                delay_secs: Some(delay_secs),
                ..ClipboardArgs::default()
            },
        );
        let clear = self.settings.clear_clipboard_after;
        let (proxy, busy) = (self.proxy.clone(), self.busy.clone());
        thread::spawn(move || {
            let outcome =
                clipboard::type_clipboard(&opts, clear, &CancelToken::default(), &mut |_| {});
            busy.store(false, Ordering::SeqCst);
            forward(&proxy, UserEvent::JobDone(outcome));
        });
    }

    fn open_window(&self) {
        if instance::send(Role::Gui, Message::Show).is_err()
            && let Ok(exe) = std::env::current_exe()
        {
            let _ = Command::new(exe).spawn();
        }
    }
}

impl ApplicationHandler<UserEvent> for Agent {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::Init) {
            self.init();
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.open_window(),
            UserEvent::Tray(_) => {}
            UserEvent::Menu(e) if e.id == self.quit_id => event_loop.exit(),
            UserEvent::Menu(e) if e.id == self.open_id => self.open_window(),
            UserEvent::Menu(e) if e.id == *self.type_item.id() => {
                // After clicking the menu, give the user time to click into the target.
                self.type_clipboard(self.settings.clipboard_delay_secs.max(1));
            }
            UserEvent::Menu(_) => {}
            UserEvent::HotKey(e) => {
                if e.state == HotKeyState::Pressed && self.hotkey.is_some_and(|h| h.id() == e.id) {
                    // The focus is already on the target window.
                    self.type_clipboard(0);
                }
            }
            UserEvent::Ipc(Message::Quit) => event_loop.exit(),
            UserEvent::Ipc(Message::Show) => self.open_window(),
            UserEvent::Ipc(Message::Reload) => {
                self.settings = settings::load();
                self.register_hotkey();
            }
            UserEvent::JobDone(outcome) => {
                self.set_tooltip(TITLE);
                if let Outcome::Failed(msg) = outcome {
                    platform::notify(TITLE, &format!("Typing failed: {msg}"), true);
                }
            }
        }
    }
}
