//! The window (eframe/egui).
//!
//! Threads: the UI runs on the main thread; each typing job runs on a worker thread and reports
//! [`JobEvent`]s over a channel; a second-launch "show" arrives from the IPC thread. Both call
//! `request_repaint`, which makes eframe run [`App::logic`] even while the window is minimized.

use std::{
    process::Command,
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Button, CollapsingHeader, ComboBox, DragValue, Id, Key, Layout, Modifiers,
    RichText, TextEdit, ViewportCommand, text_edit::TextEditState,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    autostart, clipboard, install,
    instance::{self, Instance, Message, Role},
    job::{CancelToken, JobEvent, Mode, Options, Outcome},
    settings::{self, MAX_COUNTDOWN_SECS, MAX_KEY_DELAY_MS, Settings},
    text, typing,
};

const TITLE: &str = "Auto Typer";
const TEXT_ID: &str = "autotyper-text";
const SPEED_PRESETS: [(&str, u32); 3] = [("Fast", 5), ("Normal", 20), ("Safe", 60)];

/// Opens the window and blocks until it is closed. Returns the process exit code.
pub fn run(settings: Settings) -> i32 {
    let instance = match instance::claim(Role::Gui) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            // Already open: bring that window to the front instead.
            return match instance::send(Role::Gui, Message::Show) {
                Ok(()) => 0,
                Err(e) => {
                    crate::platform::notify(
                        TITLE,
                        &format!("Auto Typer is already open ({e})."),
                        true,
                    );
                    1
                }
            };
        }
        Err(e) => {
            eprintln!("autotyper: single-instance lock unavailable: {e}");
            return run_window(settings, None);
        }
    };
    run_window(settings, Some(instance))
}

fn run_window(settings: Settings, instance: Option<Instance>) -> i32 {
    let icon = egui::IconData {
        rgba: include_bytes!(concat!(env!("OUT_DIR"), "/icon48.rgba")).to_vec(),
        width: 48,
        height: 48,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_app_id("autotyper")
            .with_icon(icon)
            .with_inner_size([480.0, 370.0])
            .with_min_inner_size([400.0, 320.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            let (ipc_tx, ipc_rx) = channel();
            if let Some(instance) = &instance {
                let ctx = cc.egui_ctx.clone();
                let _ = instance.listen(Role::Gui, move |m| {
                    let _ = ipc_tx.send(m);
                    ctx.request_repaint();
                });
            }
            Ok(Box::new(AutoTyperApp::new(settings, ipc_rx, instance)))
        }),
    );
    match result {
        Ok(()) => 0,
        Err(e) => {
            crate::platform::notify(TITLE, &format!("Cannot open the window: {e}"), true);
            1
        }
    }
}

struct RunningJob {
    cancel: CancelToken,
    events: Receiver<JobEvent>,
    worker: Option<JoinHandle<()>>,
}

struct AutoTyperApp {
    settings: Settings,
    saved: Settings,
    changed_at: Option<Instant>,
    text: String,
    show_text: bool,
    job: Option<RunningJob>,
    status: String,
    status_is_error: bool,
    title: String,
    ipc: Receiver<Message>,
    focus_pending: bool,
    autostart: bool,
    _instance: Option<Instance>,
}

impl AutoTyperApp {
    fn new(settings: Settings, ipc: Receiver<Message>, instance: Option<Instance>) -> Self {
        AutoTyperApp {
            saved: settings.clone(),
            settings,
            changed_at: None,
            text: String::new(),
            show_text: false,
            job: None,
            status: "Paste the text, press Start, then click into the target window.".into(),
            status_is_error: false,
            title: TITLE.into(),
            ipc,
            focus_pending: false,
            autostart: autostart::is_enabled(),
            _instance: instance,
        }
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }

    fn set_title(&mut self, ctx: &egui::Context, title: String) {
        if self.title != title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn paste(&mut self, ctx: &egui::Context) {
        match clipboard::read() {
            Ok(text) => {
                self.clear_text(ctx);
                self.text.push_str(&text);
                self.set_status("Pasted from the clipboard.", false);
            }
            Err(e) => self.set_status(capitalize(&e), true),
        }
    }

    /// Clears the text and egui's undo history (which keeps copies of it).
    fn clear_text(&mut self, ctx: &egui::Context) {
        self.text.zeroize();
        let id = Id::unique(TEXT_ID);
        if let Some(mut state) = TextEditState::load(ctx, id) {
            state.clear_undoer();
            state.store(ctx, id);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        if text::tokenize(&self.text, self.settings.trim_trailing_newline).is_empty() {
            self.set_status("Enter or paste some text first.", true);
            return;
        }
        let text = Zeroizing::new(self.text.clone());
        let opts = Options::from_settings(&self.settings, self.settings.countdown_secs);
        let cancel = CancelToken::default();
        let (tx, rx) = channel();
        let worker = spawn_worker(ctx.clone(), text, opts, cancel.clone(), tx);
        self.job = Some(RunningJob {
            cancel,
            events: rx,
            worker: Some(worker),
        });
        if self.settings.minimize_on_start {
            // Minimizing hands the focus back to the window used before us.
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }

    fn paste_and_start(&mut self, ctx: &egui::Context) {
        self.paste(ctx);
        if !self.status_is_error {
            self.start(ctx);
        }
    }

    fn stop(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancel.cancel();
            if let Some(worker) = job.worker.take() {
                let _ = worker.join();
            }
        }
        self.job = None;
    }

    /// Handles everything that can happen while the window is hidden or minimized.
    fn handle_background_events(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.ipc.try_recv() {
            match message {
                Message::Show => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    self.focus_pending = true;
                }
                Message::Quit => {
                    self.stop();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                Message::Reload => {
                    self.settings = settings::load();
                    self.saved = self.settings.clone();
                }
            }
        }
        if self.focus_pending && ctx.input(|i| i.viewport().minimized) != Some(true) {
            // Restore first, focus on a later tick: Focus is ignored while minimized.
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.focus_pending = false;
        }

        let events: Vec<JobEvent> = match &self.job {
            Some(job) => job.events.try_iter().collect(),
            None => vec![],
        };
        for event in events {
            match event {
                JobEvent::Countdown(n) => {
                    self.set_status(
                        format!("Starting in {n} s: click into the target window. Esc cancels."),
                        false,
                    );
                    self.set_title(ctx, format!("{TITLE} - starting in {n}"));
                }
                JobEvent::Started { total } => {
                    self.set_status(
                        format!("Typing 0 / {total}... Esc or a mouse move stops it."),
                        false,
                    );
                }
                JobEvent::Progress { done, total } => {
                    let pct = done * 100 / total.max(1);
                    self.set_status(
                        format!("Typing {done} / {total}... Esc or a mouse move stops it."),
                        false,
                    );
                    self.set_title(ctx, format!("{TITLE} - typing {pct}%"));
                }
                JobEvent::Finished(outcome) => self.finish(ctx, outcome),
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context, outcome: Outcome) {
        self.stop();
        let done = matches!(outcome, Outcome::Done { .. });
        match &outcome {
            Outcome::Done { typed } => {
                self.clear_text(ctx);
                self.set_status(
                    format!("Done: typed {} keys.", text::thousands(*typed)),
                    false,
                );
                self.set_title(ctx, format!("{TITLE} - done"));
            }
            Outcome::Cancelled { .. } => {
                self.set_status(format!("{outcome}. The text is still here."), true);
                self.set_title(ctx, format!("{TITLE} - cancelled"));
            }
            Outcome::Failed(msg) => {
                self.set_status(format!("Failed: {msg}"), true);
                self.set_title(ctx, format!("{TITLE} - failed"));
            }
        }
        // Never send Focus here: winit fakes an Alt key press for it, which would land in the
        // target window. Restoring is enough.
        if !done || self.settings.restore_after_typing {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        }
    }

    fn save_settings_when_idle(&mut self) {
        if self.settings != self.saved {
            self.changed_at.get_or_insert_with(Instant::now);
        }
        if self
            .changed_at
            .is_some_and(|t| t.elapsed() > Duration::from_millis(500))
        {
            self.changed_at = None;
            let tray_changed = self.settings.tray != self.saved.tray;
            let hotkey_changed = self.settings.hotkey != self.saved.hotkey
                || self.settings.hotkey_enabled != self.saved.hotkey_enabled;
            if let Err(e) = self.settings.save() {
                self.set_status(format!("Cannot save settings: {e}"), true);
            }
            self.saved = self.settings.clone();
            if tray_changed {
                self.apply_tray();
            } else if hotkey_changed {
                let _ = instance::send(Role::Agent, Message::Reload);
            }
        }
    }

    fn apply_tray(&mut self) {
        if self.settings.tray {
            if !instance::is_running(Role::Agent) {
                match std::env::current_exe()
                    .and_then(|exe| Command::new(exe).arg("--background").spawn())
                {
                    Ok(_) => self.set_status("The tray icon is running.", false),
                    Err(e) => self.set_status(format!("Cannot start the tray icon: {e}"), true),
                }
            }
        } else {
            let _ = instance::send(Role::Agent, Message::Quit);
            if self.autostart {
                let _ = autostart::disable();
                self.autostart = false;
            }
        }
    }

    // ------------------------------------------------------------------------- drawing

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.job.is_some();

        // Shortcuts, consumed before the text box can turn them into a line break / paste.
        let (start_key, paste_start_key) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::V),
            )
        });
        if !busy && paste_start_key {
            self.paste_and_start(&ctx);
        } else if !busy && start_key {
            self.start(&ctx);
        }

        ui.horizontal(|ui| {
            ui.heading("Text to type");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!busy, Button::new("Clear")).clicked() {
                    self.clear_text(&ctx);
                }
                let eye = if self.show_text { "Hide" } else { "Show" };
                if ui.button(eye).clicked() {
                    self.show_text = !self.show_text;
                }
                if ui
                    .add_enabled(!busy, Button::new("Paste"))
                    .on_hover_text("Replace the text with the clipboard (or double-click the box)")
                    .clicked()
                {
                    self.paste(&ctx);
                }
            });
        });

        let response = egui::ScrollArea::vertical()
            .max_height(150.0)
            .show(ui, |ui| {
                ui.add_enabled(
                    !busy,
                    TextEdit::multiline(&mut self.text)
                        .id(Id::unique(TEXT_ID))
                        .password(!self.show_text)
                        .hint_text("Paste or type here. Double-click pastes the clipboard.")
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        if response.double_clicked() && !busy {
            self.paste(&ctx);
        }
        let (chars, lines) = text::stats(&self.text);
        ui.label(
            RichText::new(format!(
                "{} chars · {} {}",
                text::thousands(chars),
                lines,
                if lines == 1 { "line" } else { "lines" }
            ))
            .small()
            .weak(),
        );
        ui.add_space(6.0);

        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("options")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Countdown");
                    ui.add(
                        DragValue::new(&mut self.settings.countdown_secs)
                            .range(0..=MAX_COUNTDOWN_SECS)
                            .suffix(" s"),
                    );
                    ui.end_row();

                    ui.label("Speed");
                    ui.horizontal(|ui| {
                        for (name, ms) in SPEED_PRESETS {
                            ui.selectable_value(&mut self.settings.key_delay_ms, ms, name);
                        }
                        ui.add(
                            DragValue::new(&mut self.settings.key_delay_ms)
                                .range(0..=MAX_KEY_DELAY_MS)
                                .suffix(" ms/key"),
                        );
                    });
                    ui.end_row();

                    ui.label("Input method");
                    ComboBox::from_id_salt("mode")
                        .selected_text(self.settings.mode.label())
                        .show_ui(ui, |ui| {
                            for mode in Mode::ALL {
                                ui.selectable_value(&mut self.settings.mode, mode, mode.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "Unicode works almost everywhere, including RDP lock screens.\n\
                         Key presses: for VM consoles and web gateways that ignore Unicode;\n\
                         the remote keyboard layout must match yours.",
                        );
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.settings.press_enter, "Press Enter after the text");
                    ui.end_row();
                });
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let wide = [ui.available_width() * 0.55, 34.0];
            if busy {
                if ui
                    .add_sized(wide, Button::new(RichText::new("Stop").strong()))
                    .clicked()
                {
                    self.stop();
                    self.set_status("Stopped.", true);
                    self.set_title(&ctx, TITLE.into());
                }
            } else {
                let start = ui
                    .add_sized(wide, Button::new(RichText::new("Start typing").strong()))
                    .on_hover_text("Ctrl+Enter");
                if start.clicked() {
                    self.start(&ctx);
                }
                if ui
                    .add_sized([ui.available_width(), 34.0], Button::new("Paste & start"))
                    .on_hover_text("Replace the text with the clipboard, then start (Ctrl+Shift+V)")
                    .clicked()
                {
                    self.paste_and_start(&ctx);
                }
            }
        });
        ui.add_space(4.0);
        let status = RichText::new(&self.status);
        ui.label(if self.status_is_error {
            status.color(ui.visuals().warn_fg_color)
        } else {
            status
        });
        ui.add_space(4.0);

        CollapsingHeader::new("More options").show(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| self.draw_more(ui));
        });
    }

    fn draw_more(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.checkbox(
            &mut s.minimize_on_start,
            "Minimize on Start (gives the focus back to the previous window)",
        );
        ui.checkbox(
            &mut s.restore_after_typing,
            "Bring the window back when done",
        );
        ui.checkbox(&mut s.abort_on_mouse_move, "Stop when the mouse moves");
        ui.checkbox(
            &mut s.abort_on_focus_change,
            "Stop when another window gets the focus",
        );
        ui.checkbox(
            &mut s.trim_trailing_newline,
            "Ignore one trailing line break",
        );
        ui.horizontal(|ui| {
            ui.label("Type-clipboard delay");
            ui.add(
                DragValue::new(&mut s.clipboard_delay_secs)
                    .range(0..=MAX_COUNTDOWN_SECS)
                    .suffix(" s"),
            );
        });
        ui.checkbox(
            &mut s.clear_clipboard_after,
            "Clear the clipboard after typing it",
        );

        ui.separator();
        ui.checkbox(
            &mut s.tray,
            "Keep a tray icon running (type the clipboard from anywhere)",
        );
        ui.add_enabled_ui(s.tray, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.hotkey_enabled, "Hotkey");
                ui.add(TextEdit::singleline(&mut s.hotkey).desired_width(160.0))
                    .on_hover_text("For example Ctrl+Alt+Shift+V or Ctrl+Shift+F9");
            });
            let exe = install::installed_exe()
                .filter(|p| p.exists())
                .or_else(|| std::env::current_exe().ok());
            let mut on = self.autostart;
            if ui
                .checkbox(&mut on, "Start the tray icon when I sign in")
                .changed()
            {
                let result = match (&exe, on) {
                    (Some(exe), true) => autostart::enable(exe),
                    (_, false) => autostart::disable(),
                    (None, true) => Err("cannot find the program file".into()),
                };
                match result {
                    Ok(()) => self.autostart = on,
                    Err(e) => {
                        self.status = format!("Autostart: {e}");
                        self.status_is_error = true;
                    }
                }
            }
        });

        ui.separator();
        if cfg!(any(windows, target_os = "linux")) {
            if install::running_installed_copy() {
                ui.horizontal(|ui| {
                    ui.label("Installed for this user.");
                    if ui.button("Uninstall...").clicked()
                        && let Ok(exe) = std::env::current_exe()
                    {
                        let _ = Command::new(exe).arg("--uninstall").spawn();
                    }
                });
            } else {
                let label = if install::is_installed() {
                    "Update the installed copy"
                } else {
                    "Install"
                };
                if ui
                    .button(label)
                    .on_hover_text("Copies Auto Typer to your user profile and adds Start menu entries, so you can launch it from Windows Search or PowerToys Command Palette.")
                    .clicked()
                {
                    match install::install() {
                        Ok(msg) => self.set_status(msg, false),
                        Err(e) => self.set_status(format!("Install failed: {e}"), true),
                    }
                }
            }
        }
        ui.label(
            RichText::new(format!(
                "Auto Typer {} · github.com/viru185/AutoTyper",
                env!("CARGO_PKG_VERSION")
            ))
            .small()
            .weak(),
        );
    }
}

impl eframe::App for AutoTyperApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_background_events(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            // Make sure no key is left half-typed.
            self.stop();
        }
        self.save_settings_when_idle();
        if self.changed_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(600));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| self.draw(ui));
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop();
        self.text.zeroize();
        if self.settings != self.saved {
            let _ = self.settings.save();
        }
    }
}

fn spawn_worker(
    ctx: egui::Context,
    text: Zeroizing<String>,
    opts: Options,
    cancel: CancelToken,
    tx: Sender<JobEvent>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("typing".into())
        .spawn(move || {
            typing::run(&text, &opts, &cancel, &mut |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            });
        })
        .expect("spawn the typing thread")
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
