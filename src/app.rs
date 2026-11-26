//! The window (eframe/egui): a small always-on-top box with the text and a Start button;
//! everything else lives behind the settings toggle.
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
    self, Align, Button, ComboBox, DragValue, Id, Key, Layout, Modifiers, ProgressBar, RichText,
    TextEdit, ViewportCommand, WindowLevel, text_edit::TextEditState,
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
const WIDTH: f32 = 400.0;
/// Vertical inner margin of the central panel (top + bottom) plus a little slack.
const PANEL_MARGIN: f32 = 18.0;
const STATUS_HEIGHT: f32 = 18.0;

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
            .with_window_level(window_level(settings.always_on_top))
            // The height is fitted to the content on the first frame.
            .with_inner_size([WIDTH, 180.0])
            .with_min_inner_size([320.0, 120.0]),
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

fn window_level(always_on_top: bool) -> WindowLevel {
    if always_on_top {
        WindowLevel::AlwaysOnTop
    } else {
        WindowLevel::Normal
    }
}

struct RunningJob {
    cancel: CancelToken,
    events: Receiver<JobEvent>,
    worker: Option<JoinHandle<()>>,
    countdown: Option<u32>,
    progress: Option<(usize, usize)>,
}

struct AutoTyperApp {
    settings: Settings,
    saved: Settings,
    changed_at: Option<Instant>,
    text: String,
    show_text: bool,
    show_settings: bool,
    job: Option<RunningJob>,
    status: String,
    status_is_error: bool,
    title: String,
    ipc: Receiver<Message>,
    focus_pending: bool,
    /// We minimized ourselves to hand the focus back; restore when the job ends.
    minimized_for_job: bool,
    /// Our window handle, captured when Start is clicked (Windows).
    #[cfg(windows)]
    own_window: Option<isize>,
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
            show_settings: false,
            job: None,
            status: "Ready".into(),
            status_is_error: false,
            title: TITLE.into(),
            ipc,
            focus_pending: false,
            minimized_for_job: false,
            #[cfg(windows)]
            own_window: None,
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
                self.set_status("Pasted from the clipboard", false);
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
            self.set_status("Paste or type some text first", true);
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
            countdown: None,
            progress: None,
        });
        self.set_status("Starting...", false);
        if self.settings.return_focus_on_start {
            self.return_focus(ctx);
        }
    }

    /// Hands the keyboard focus back to the window the user came from (e.g. the Remote
    /// Desktop session), so typing goes there.
    fn return_focus(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            use crate::platform::windows as win;
            // Stay visible (and on top) to show the countdown and progress.
            self.own_window = win::own_foreground_window();
            if let Some(own) = self.own_window
                && win::focus_previous_window(own)
            {
                return;
            }
        }
        // Fallback: minimizing makes the OS activate the previous window.
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        self.minimized_for_job = true;
    }

    fn restore_after_job(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.minimized_for_job) {
            return;
        }
        // Never send ViewportCommand::Focus here: winit fakes an Alt key press for it, which
        // would land in the target window.
        #[cfg(windows)]
        if let Some(own) = self.own_window {
            // Shows the window again without taking the focus from the target.
            crate::platform::windows::show_without_activating(own);
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
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
                    if let Some(job) = &mut self.job {
                        job.countdown = Some(n);
                    }
                    self.set_status(format!("Typing starts in {n} s · Esc cancels"), false);
                    self.set_title(ctx, format!("{TITLE} - starting in {n}"));
                }
                JobEvent::Started { total } => {
                    if let Some(job) = &mut self.job {
                        job.countdown = None;
                        job.progress = Some((0, total));
                    }
                }
                JobEvent::Progress { done, total } => {
                    if let Some(job) = &mut self.job {
                        job.progress = Some((done, total));
                    }
                    let pct = done * 100 / total.max(1);
                    self.set_title(ctx, format!("{TITLE} - typing {pct}%"));
                }
                JobEvent::Finished(outcome) => self.finish(ctx, outcome),
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context, outcome: Outcome) {
        self.stop();
        match &outcome {
            Outcome::Done { typed } => {
                self.clear_text(ctx);
                self.set_status(
                    format!("Done · typed {} keys", text::thousands(*typed)),
                    false,
                );
                self.set_title(ctx, format!("{TITLE} - done"));
            }
            Outcome::Cancelled { .. } => {
                self.set_status(format!("{outcome} · the text is kept"), true);
                self.set_title(ctx, format!("{TITLE} - cancelled"));
            }
            Outcome::Failed(msg) => {
                self.set_status(format!("Failed: {msg}"), true);
                self.set_title(ctx, format!("{TITLE} - failed"));
            }
        }
        self.restore_after_job(ctx);
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
                    Ok(_) => self.set_status("The tray icon is running", false),
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

        // The text.
        let response = egui::ScrollArea::vertical()
            .id_salt("text")
            .max_height(96.0)
            .show(ui, |ui| {
                ui.add_enabled(
                    !busy,
                    TextEdit::multiline(&mut self.text)
                        .id(Id::unique(TEXT_ID))
                        .password(!self.show_text)
                        .hint_text("Paste or type the text to type")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        if response.double_clicked() && !busy {
            self.paste(&ctx);
        }

        // Counts on the left, small icon buttons on the right.
        ui.horizontal(|ui| {
            let (chars, lines) = text::stats(&self.text);
            let info = if chars == 0 {
                "Double-click the box to paste".to_string()
            } else {
                format!(
                    "{} chars · {} {}",
                    text::thousands(chars),
                    lines,
                    if lines == 1 { "line" } else { "lines" }
                )
            };
            ui.label(RichText::new(info).small().weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if toggle(ui, self.show_settings, Icon::Gear, "Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                if toggle(ui, self.settings.always_on_top, Icon::Pin, "Keep on top").clicked() {
                    self.settings.always_on_top = !self.settings.always_on_top;
                    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(window_level(
                        self.settings.always_on_top,
                    )));
                }
                ui.separator();
                ui.add_enabled_ui(!busy, |ui| {
                    if icon(ui, Icon::Clear, "Clear").clicked() {
                        self.clear_text(&ctx);
                        self.set_status("Ready", false);
                    }
                    if icon(ui, Icon::Paste, "Paste from the clipboard").clicked() {
                        self.paste(&ctx);
                    }
                });
                let eye = if self.show_text {
                    "Hide the text"
                } else {
                    "Show the text"
                };
                if toggle(ui, self.show_text, Icon::Eye, eye).clicked() {
                    self.show_text = !self.show_text;
                }
            });
        });

        // Start / Stop.
        ui.horizontal(|ui| {
            let height = 30.0;
            if let Some(job) = &self.job {
                let label = match job.countdown {
                    Some(n) => format!("Stop  ·  starts in {n}"),
                    None => "Stop".into(),
                };
                if ui
                    .add_sized([ui.available_width(), height], Button::new(label))
                    .on_hover_text("Esc also stops")
                    .clicked()
                {
                    self.stop();
                    self.set_status("Stopped", true);
                    self.set_title(&ctx, TITLE.into());
                }
            } else {
                let start = Button::new(RichText::new("Start").strong())
                    .fill(ui.visuals().selection.bg_fill);
                if ui
                    .add_sized([ui.available_width() * 0.6, height], start)
                    .on_hover_text("Start typing (Ctrl+Enter)")
                    .clicked()
                {
                    self.start(&ctx);
                }
                if ui
                    .add_sized([ui.available_width(), height], Button::new("Paste & start"))
                    .on_hover_text("Replace the text with the clipboard, then start (Ctrl+Shift+V)")
                    .clicked()
                {
                    self.paste_and_start(&ctx);
                }
            }
        });

        // Status line, or progress while typing.
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(width, STATUS_HEIGHT),
            Layout::top_down(Align::Min),
            |ui| {
                if let Some((done, total)) = self.job.as_ref().and_then(|j| j.progress) {
                    ui.add(
                        ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_height(STATUS_HEIGHT)
                            .text(format!("{done} / {total} · Esc stops")),
                    );
                } else {
                    let status = RichText::new(&self.status).small();
                    ui.label(if self.status_is_error {
                        status.color(ui.visuals().warn_fg_color)
                    } else {
                        status.weak()
                    });
                }
            },
        );

        if self.show_settings {
            ui.separator();
            ui.add_enabled_ui(!busy, |ui| self.draw_settings(ui));
        }
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Countdown");
                ui.add(
                    DragValue::new(&mut s.countdown_secs)
                        .range(0..=MAX_COUNTDOWN_SECS)
                        .suffix(" s"),
                );
                ui.end_row();

                ui.label("Speed");
                ui.horizontal(|ui| {
                    for (name, ms) in SPEED_PRESETS {
                        ui.selectable_value(&mut s.key_delay_ms, ms, name);
                    }
                    ui.add(
                        DragValue::new(&mut s.key_delay_ms)
                            .range(0..=MAX_KEY_DELAY_MS)
                            .suffix(" ms"),
                    )
                    .on_hover_text("Pause between keystrokes");
                });
                ui.end_row();

                ui.label("Input");
                ComboBox::from_id_salt("mode")
                    .selected_text(s.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in Mode::ALL {
                            ui.selectable_value(&mut s.mode, mode, mode.label());
                        }
                    })
                    .response
                    .on_hover_text(
                        "Unicode works almost everywhere, including RDP lock screens.\n\
                         Key presses: for VM consoles and web gateways that ignore Unicode;\n\
                         the remote keyboard layout must match yours.",
                    );
                ui.end_row();

                ui.label("Clipboard delay");
                ui.add(
                    DragValue::new(&mut s.clipboard_delay_secs)
                        .range(0..=MAX_COUNTDOWN_SECS)
                        .suffix(" s"),
                )
                .on_hover_text("Wait before \"Type clipboard\" (Start menu, tray, --clipboard)");
                ui.end_row();
            });
        ui.checkbox(&mut s.press_enter, "Press Enter after the text");
        ui.checkbox(
            &mut s.return_focus_on_start,
            "Give the focus back to the previous window on Start",
        );
        ui.checkbox(&mut s.abort_on_mouse_move, "Stop when the mouse moves");
        ui.checkbox(
            &mut s.abort_on_focus_change,
            "Stop when another window gets the focus",
        );
        ui.checkbox(&mut s.trim_trailing_newline, "Ignore a trailing line break");
        ui.checkbox(
            &mut s.clear_clipboard_after,
            "Clear the clipboard after typing it",
        );

        ui.separator();
        ui.checkbox(&mut s.tray, "Tray icon (type the clipboard from anywhere)");
        ui.add_enabled_ui(s.tray, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.hotkey_enabled, "Hotkey");
                ui.add(TextEdit::singleline(&mut s.hotkey).desired_width(150.0))
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
        ui.horizontal(|ui| {
            if cfg!(any(windows, target_os = "linux")) {
                if install::running_installed_copy() {
                    if ui.button("Uninstall...").clicked()
                        && let Ok(exe) = std::env::current_exe()
                    {
                        let _ = Command::new(exe).arg("--uninstall").spawn();
                    }
                } else {
                    let label = if install::is_installed() {
                        "Update installed copy"
                    } else {
                        "Install"
                    };
                    if ui
                        .button(label)
                        .on_hover_text(
                            "Copies Auto Typer to your user profile and adds Start menu \
                             entries, so you can launch it from Windows Search or PowerToys \
                             Command Palette.",
                        )
                        .clicked()
                    {
                        match install::install() {
                            Ok(msg) => self.set_status(msg, false),
                            Err(e) => self.set_status(format!("Install failed: {e}"), true),
                        }
                    }
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                )
                .on_hover_text("github.com/viru185/AutoTyper");
            });
        });
    }

    /// Keeps the window exactly as tall as its content (the width is up to the user).
    fn fit_height(&self, ctx: &egui::Context, content_height: f32) {
        let (inner, minimized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().minimized));
        let Some(inner) = inner else { return };
        if minimized == Some(true) || inner.height() < 1.0 {
            return;
        }
        let wanted = (content_height + PANEL_MARGIN).ceil();
        if (wanted - inner.height()).abs() > 2.0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(
                inner.width(),
                wanted,
            )));
        }
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
        let content_height = egui::CentralPanel::default()
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.draw(ui))
                    .content_size
                    .y
            })
            .inner;
        self.fit_height(ui.ctx(), content_height);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop();
        self.text.zeroize();
        if self.settings != self.saved {
            let _ = self.settings.save();
        }
    }
}

/// Small icons, drawn with vector shapes: the default fonts have no symbol glyphs, and an
/// emoji font would add hundreds of KB to the binary.
#[derive(Clone, Copy)]
enum Icon {
    Eye,
    Paste,
    Clear,
    Pin,
    Gear,
}

/// A frameless icon button.
fn icon(ui: &mut egui::Ui, icon: Icon, tip: &str) -> egui::Response {
    icon_button(ui, icon, false, tip)
}

/// An icon button that is highlighted while `on`.
fn toggle(ui: &mut egui::Ui, on: bool, icon: Icon, tip: &str) -> egui::Response {
    icon_button(ui, icon, on, tip)
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, tip: &str) -> egui::Response {
    use egui::{Pos2, Sense, Shape, Stroke, vec2};

    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let painter = ui.painter();
        if selected || response.hovered() {
            painter.rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = visuals.fg_stroke.color;
        let stroke = Stroke::new(1.4, color);
        let c = rect.center();
        let r = 7.0;
        match icon {
            Icon::Eye => {
                painter.add(Shape::ellipse_stroke(c, vec2(r, r * 0.6), stroke));
                painter.circle_filled(c, r * 0.3, color);
            }
            Icon::Paste => {
                let board =
                    egui::Rect::from_center_size(c + vec2(0.0, 1.0), vec2(r * 1.4, r * 1.7));
                painter.rect_stroke(board, 1.5, stroke, egui::StrokeKind::Inside);
                let clip = egui::Rect::from_center_size(
                    Pos2::new(c.x, board.top()),
                    vec2(r * 0.7, r * 0.45),
                );
                painter.rect_filled(clip, 1.0, color);
            }
            Icon::Clear => {
                let d = r * 0.6;
                painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
                painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
            }
            Icon::Pin => {
                // A push pin: head, collar and needle.
                let head = egui::Rect::from_min_max(
                    Pos2::new(c.x - r * 0.4, c.y - r),
                    Pos2::new(c.x + r * 0.4, c.y - r * 0.1),
                );
                painter.rect_filled(head, 1.5, color);
                painter.line_segment(
                    [
                        Pos2::new(c.x - r * 0.75, c.y - r * 0.1),
                        Pos2::new(c.x + r * 0.75, c.y - r * 0.1),
                    ],
                    stroke,
                );
                painter.line_segment([Pos2::new(c.x, c.y), Pos2::new(c.x, c.y + r)], stroke);
            }
            Icon::Gear => {
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let dir = vec2(a.cos(), a.sin());
                    painter.line_segment(
                        [c + dir * r * 0.55, c + dir * r * 0.95],
                        Stroke::new(2.6, color),
                    );
                }
                painter.circle_stroke(c, r * 0.6, Stroke::new(2.0, color));
                painter.circle_filled(c, r * 0.2, color);
            }
        }
    }
    response.on_hover_text(tip)
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
