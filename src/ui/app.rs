//! Homepage: invisible root-level scan capture, prompts, notices, QR materialization,
//! raw-data box, reprint, counter instructions, banners/toasts and the version footer.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CursorIcon, Event, Frame, Key, Label, Layout,
    Margin, Mesh, RichText, ScrollArea, Sense, Shadow, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use postal_annex_qr_scanner::clipboard;
use postal_annex_qr_scanner::config::AppConfig;
use postal_annex_qr_scanner::domain::{
    AlertBadge, AppEffect, PrintJob, PrintTrigger, PromptKind, ScanBuffer, ScannerStateMachine,
    ScreenState,
};
use postal_annex_qr_scanner::installer;
use postal_annex_qr_scanner::printer::{PrintEvent, PrintWorker};
use postal_annex_qr_scanner::qr::{
    self,
    animation::{self, QrAnimation},
    QrMatrix, QUIET_ZONE_MODULES,
};
use postal_annex_qr_scanner::updater::{
    self, AvailableUpdate, UpdateEvent, UpdateWorker, CURRENT_VERSION,
};

use super::colors::*;
use super::{
    blink_phase, boxed, card_width, chrome, close_icon_button, flat_button, font_bold,
    font_bold_italic, font_mono, font_regular, font_semibold, install_theme, overlay_button,
    paint_dashed_rule, paint_gear, secondary_button_style, settings, snap_to_pixel,
    text_button_width, text_format, whats_new, ButtonStyle, FontSupport, COPIED_FEEDBACK,
    CONFIG_SAVE_DEBOUNCE, QR_PREVIEW_SIZE,
};

const HAPPY_RETURN_QUESTION: &str =
    "This appears to be a Happy Return, have you already tried accepting through PostalMate?";
const CUSTOMER_PACKED_QUESTION: &str =
    "This item is marked as customer packed. Has it been properly packed yet?";
const HAPPY_RETURN_NOTE: &str = "Please try this return in PostalMate first.";
const PACKING_NOTICE: &str = "Please charge customer for appropriate packing materials.";
const UNRECOGNIZED_WARNING: &str =
    "Please check the return location of this QR code before accepting it.";
const COUNTER_INSTRUCTIONS: [&str; 3] = [
    "1. Scan QR code while app shows \"Ready to Scan\"",
    "2. Label prints automatically to configured printer \u{2014} place on item",
    "3. Press CTRL + V in PostalMate to automatically paste RMA number",
];

/// Encoded QR matrix plus its materialization animation clock.
pub(super) struct CachedQr {
    pub matrix: QrMatrix,
    pub anim: QrAnimation,
}

/// Who queued a print job (events come back from the worker in FIFO order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrintOrigin {
    Scan,
    Test,
}

/// Inline feedback for the Settings "Test Print" button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TestPrintStatus {
    Idle,
    Sending,
    Sent,
    Failed(String),
}

/// UI-side view of the auto-updater lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading { downloaded: u64, total: Option<u64> },
    Verifying,
    ReadyToRestart,
    Error(String),
}

/// A dismissible, non-blocking red toast.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ErrorToast {
    title: &'static str,
    message: String,
}

/// Deferred banner / toast actions (applied after drawing to keep borrows simple).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BannerAction {
    InstallUpdate,
    DismissUpdate,
    AddDesktopShortcut,
    DismissInstalled,
    OpenSettings,
    DismissPrintError,
    DismissMiscError,
}

/// Deferred card actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardAction {
    OpenSettings,
    Confirm(bool),
    Override,
    CopyAll,
    Reprint,
    Reset,
}

/// The PostalAnnex+ Amazon QR Code Scanner application.
pub struct ScannerApp {
    pub(super) config: AppConfig,
    state: ScannerStateMachine,
    scan_buffer: ScanBuffer,
    pub(super) print_worker: PrintWorker,
    pub(super) update_worker: UpdateWorker,

    cached_qr: Option<CachedQr>,
    qr_error: Option<String>,
    copied_until: Option<Instant>,
    screen_changed_at: Instant,
    pending_prints: VecDeque<PrintOrigin>,

    pub(super) settings_open: bool,
    pub(super) whats_new_open: bool,
    pub(super) whats_new_notes: String,
    pub(super) printers: Vec<String>,
    pub(super) printers_refreshing: bool,
    pub(super) printers_error: Option<String>,
    pub(super) test_print: TestPrintStatus,
    pub(super) config_dirty_at: Option<Instant>,
    pub(super) config_error: Option<String>,

    pub(super) update_phase: UpdatePhase,
    pub(super) available_update: Option<AvailableUpdate>,
    pub(super) installing_update: bool,
    update_banner_dismissed: bool,
    installed_banner_visible: bool,
    pub(super) shortcut_feedback: Option<(bool, String)>,

    print_error: Option<String>,
    misc_error: Option<ErrorToast>,
    fonts: FontSupport,
}

impl ScannerApp {
    /// Builds the app: theme + fonts, config, background print/update workers.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        just_installed: bool,
        just_updated: bool,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let fonts = install_theme(&ctx);
        let (mut config, had_existing_config) = AppConfig::load_with_existing_flag();

        let auto_open_whats_new = should_auto_open_whats_new(
            just_updated,
            had_existing_config,
            config.last_seen_version.as_deref(),
            CURRENT_VERSION,
        );
        if config.last_seen_version.as_deref() != Some(CURRENT_VERSION) {
            config.last_seen_version = Some(CURRENT_VERSION.to_string());
            let _ = config.save();
        }

        let whats_new_notes = initial_changelog_notes();

        let repaint_ctx = ctx.clone();
        let repaint_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || repaint_ctx.request_repaint());
        let print_worker = PrintWorker::spawn(Some(Arc::clone(&repaint_cb)));
        let update_worker = UpdateWorker::spawn(Some(repaint_cb));

        print_worker.refresh_printers();
        if config.auto_check_updates {
            update_worker.check(config.github_repo.clone(), true, Some(Duration::from_secs(1)));
        }
        if auto_open_whats_new {
            update_worker.fetch_changelog(config.github_repo.clone(), CURRENT_VERSION.to_string());
        }

        Self {
            config,
            state: ScannerStateMachine::new(),
            scan_buffer: ScanBuffer::new(),
            print_worker,
            update_worker,
            cached_qr: None,
            qr_error: None,
            copied_until: None,
            screen_changed_at: Instant::now(),
            pending_prints: VecDeque::new(),
            settings_open: false,
            whats_new_open: auto_open_whats_new,
            whats_new_notes,
            printers: Vec::new(),
            printers_refreshing: true,
            printers_error: None,
            test_print: TestPrintStatus::Idle,
            config_dirty_at: None,
            config_error: None,
            update_phase: UpdatePhase::Idle,
            available_update: None,
            installing_update: false,
            update_banner_dismissed: false,
            installed_banner_visible: just_installed,
            shortcut_feedback: None,
            print_error: None,
            misc_error: None,
            fonts,
        }
    }

    // -----------------------------------------------------------------------
    // Actions shared with modal overlays
    // -----------------------------------------------------------------------

    fn is_modal_open(&self) -> bool {
        self.settings_open || self.whats_new_open
    }

    pub(super) fn open_settings(&mut self) {
        self.whats_new_open = false;
        self.settings_open = true;
        self.scan_buffer.clear();
        self.test_print = TestPrintStatus::Idle;
        if !self.printers_refreshing {
            self.request_printer_refresh();
        }
    }

    pub(super) fn close_settings(&mut self) {
        self.settings_open = false;
        self.scan_buffer.clear();
        self.save_config_now();
    }

    pub(super) fn open_whats_new(&mut self) {
        self.settings_open = false;
        self.whats_new_open = true;
        self.scan_buffer.clear();
        self.whats_new_notes = initial_changelog_notes();
        self.update_worker.fetch_changelog(
            self.config.github_repo.trim().to_string(),
            CURRENT_VERSION.to_string(),
        );
    }

    pub(super) fn close_whats_new(&mut self) {
        self.whats_new_open = false;
        self.scan_buffer.clear();
    }

    pub(super) fn request_printer_refresh(&mut self) {
        self.printers_refreshing = true;
        self.printers_error = None;
        self.print_worker.refresh_printers();
    }

    pub(super) fn send_test_print(&mut self) {
        self.test_print = TestPrintStatus::Sending;
        self.pending_prints.push_back(PrintOrigin::Test);
        self.print_worker
            .print(ScannerStateMachine::test_print_job(), self.config.clone());
    }

    pub(super) fn check_for_updates(&mut self) {
        self.update_phase = UpdatePhase::Checking;
        self.update_worker
            .check(self.config.github_repo.trim().to_string(), false, None);
    }

    pub(super) fn start_update_install(&mut self) {
        if self.installing_update {
            return;
        }
        if let Some(update) = self.available_update.clone() {
            self.installing_update = true;
            self.update_banner_dismissed = false;
            self.update_phase = UpdatePhase::Downloading {
                downloaded: 0,
                total: None,
            };
            self.update_worker.install(update);
        }
    }

    pub(super) fn save_config_now(&mut self) {
        if self.config_dirty_at.take().is_some() {
            match self.config.save() {
                Ok(_) => self.config_error = None,
                Err(err) => {
                    self.config_error = Some(err.clone());
                    if !self.settings_open {
                        self.misc_error = Some(ErrorToast {
                            title: "Settings could not be saved",
                            message: err,
                        });
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Worker events
    // -----------------------------------------------------------------------

    fn poll_workers(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.print_worker.try_recv() {
            match event {
                PrintEvent::PrintSucceeded { .. } => {
                    match self.pending_prints.pop_front().unwrap_or(PrintOrigin::Scan) {
                        PrintOrigin::Scan => self.print_error = None,
                        PrintOrigin::Test => self.test_print = TestPrintStatus::Sent,
                    }
                }
                PrintEvent::PrintFailed(message) => {
                    match self.pending_prints.pop_front().unwrap_or(PrintOrigin::Scan) {
                        PrintOrigin::Scan => self.print_error = Some(message),
                        PrintOrigin::Test => self.test_print = TestPrintStatus::Failed(message),
                    }
                }
                PrintEvent::PrintersLoaded(Ok(list)) => {
                    self.printers = list;
                    self.printers_refreshing = false;
                    self.printers_error = None;
                }
                PrintEvent::PrintersLoaded(Err(err)) => {
                    self.printers_refreshing = false;
                    self.printers_error = Some(err);
                }
            }
        }

        while let Some(event) = self.update_worker.try_recv() {
            self.handle_update_event(event, ctx);
        }
    }

    fn handle_update_event(&mut self, event: UpdateEvent, ctx: &egui::Context) {
        match event {
            UpdateEvent::Checking => {
                if !self.installing_update {
                    self.update_phase = UpdatePhase::Checking;
                }
            }
            UpdateEvent::UpToDate { .. } => {
                self.update_phase = UpdatePhase::UpToDate;
                self.available_update = None;
            }
            UpdateEvent::Available(update) => {
                self.update_phase = UpdatePhase::Available;
                self.available_update = Some(update);
                self.update_banner_dismissed = false;
            }
            UpdateEvent::DownloadProgress { downloaded, total } => {
                self.installing_update = true;
                self.update_phase = UpdatePhase::Downloading { downloaded, total };
            }
            UpdateEvent::Verifying => {
                self.update_phase = UpdatePhase::Verifying;
            }
            UpdateEvent::ReadyToRestart => {
                self.update_phase = UpdatePhase::ReadyToRestart;
                self.save_config_now();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            UpdateEvent::ChangelogLoaded { version, notes } => {
                let normalized = version.trim().trim_start_matches(['v', 'V']);
                if normalized == CURRENT_VERSION && !notes.trim().is_empty() {
                    self.whats_new_notes = notes.clone();
                    let _ = updater::save_cached_changelog(&version, &notes);
                }
            }
            UpdateEvent::Error { message, silent } => {
                if self.installing_update {
                    self.installing_update = false;
                    self.update_banner_dismissed = false;
                    self.update_phase = UpdatePhase::Error(message);
                } else if silent {
                    self.update_phase = UpdatePhase::Idle;
                } else {
                    self.update_phase = UpdatePhase::Error(message);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Invisible root-level scan capture engine (SPEC §5.1)
    // -----------------------------------------------------------------------

    fn capture_scan_input(&mut self, ctx: &egui::Context, now: Instant) {
        self.scan_buffer.tick(now);

        let (events, modifiers) = ctx.input(|i| (i.events.clone(), i.modifiers));
        let shortcut_held = modifiers.ctrl || modifiers.command || modifiers.alt;

        for event in events {
            match event {
                Event::Text(text) => {
                    if !shortcut_held {
                        self.scan_buffer.push_str(&text, now);
                    }
                }
                Event::Paste(text) => self.scan_buffer.push_str(&text, now),
                Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers: m,
                    ..
                } => {
                    if !(m.ctrl || m.command || m.alt) {
                        if let Some(raw) = self.scan_buffer.submit(now) {
                            self.process_submitted_scan(&raw, ctx, now);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn process_submitted_scan(&mut self, raw: &str, ctx: &egui::Context, now: Instant) {
        let effects = self.state.handle_scan(raw);
        self.on_screen_changed(now);
        self.execute_effects(effects, ctx, now);
    }

    /// Flushes per-scan visuals whenever the state machine transitions.
    fn on_screen_changed(&mut self, now: Instant) {
        self.screen_changed_at = now;
        self.copied_until = None;
        self.qr_error = None;
        self.cached_qr = None;
    }

    /// Clears all active scan state and returns the homepage to its initial Ready view.
    fn reset_to_home(&mut self, now: Instant) {
        self.state.reset();
        self.scan_buffer.clear();
        self.on_screen_changed(now);
        self.print_error = None;
        self.misc_error = None;
    }

    /// Returns `true` while scan input is arriving or the QR code is materializing.
    fn is_generating(&self, now: Instant) -> bool {
        !self.scan_buffer.is_empty()
            || self
                .cached_qr
                .as_ref()
                .is_some_and(|qr| !qr.anim.is_finished_at(now))
    }

    /// Executes effects immediately: prints are queued first (zero delay), then clipboard
    /// copy and QR generation. A clipboard failure never blocks printing and vice versa.
    fn execute_effects(&mut self, effects: Vec<AppEffect>, ctx: &egui::Context, now: Instant) {
        let (prints, others): (Vec<_>, Vec<_>) = effects
            .into_iter()
            .partition(|e| matches!(e, AppEffect::Print { .. }));

        for effect in prints.into_iter().chain(others) {
            match effect {
                AppEffect::Print { job, trigger } => self.dispatch_print(job, trigger),
                AppEffect::CopyToClipboard(text) => match clipboard::copy_text(&text) {
                    Ok(()) => {
                        if self
                            .misc_error
                            .as_ref()
                            .is_some_and(|t| t.title == CLIPBOARD_TOAST_TITLE)
                        {
                            self.misc_error = None;
                        }
                    }
                    Err(err) => {
                        self.misc_error = Some(ErrorToast {
                            title: CLIPBOARD_TOAST_TITLE,
                            message: err,
                        });
                    }
                },
                AppEffect::GenerateQr {
                    payload,
                    scan_nonce,
                } => match qr::encode(&payload) {
                    Ok(matrix) => {
                        self.cached_qr = Some(CachedQr {
                            matrix,
                            anim: QrAnimation::new(scan_nonce, now),
                        });
                        self.qr_error = None;
                        ctx.request_repaint();
                    }
                    Err(err) => {
                        self.cached_qr = None;
                        self.qr_error = Some(err);
                    }
                },
            }
        }
    }

    fn dispatch_print(&mut self, job: PrintJob, trigger: PrintTrigger) {
        // Mirrors `PrintWorker::print_with_trigger` so FIFO origin tracking stays aligned.
        if trigger == PrintTrigger::Auto && !self.config.auto_print_on_scan {
            return;
        }
        self.pending_prints.push_back(PrintOrigin::Scan);
        self.print_worker
            .print_with_trigger(job, trigger, self.config.clone());
    }

    fn last_clipboard_ok(&self) -> bool {
        !self
            .misc_error
            .as_ref()
            .is_some_and(|t| t.title == CLIPBOARD_TOAST_TITLE)
    }

    // -----------------------------------------------------------------------
    // Frame scheduling helpers
    // -----------------------------------------------------------------------

    /// Focus-trap guard: on the homepage nothing may hold keyboard focus, so a scanner's
    /// trailing `Enter` can never activate a previously clicked control.
    fn release_focus(ctx: &egui::Context) {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }

    fn flush_config_if_due(&mut self, ctx: &egui::Context, now: Instant) {
        if let Some(changed_at) = self.config_dirty_at {
            let elapsed = now.saturating_duration_since(changed_at);
            if elapsed >= CONFIG_SAVE_DEBOUNCE {
                self.save_config_now();
            } else {
                ctx.request_repaint_after(CONFIG_SAVE_DEBOUNCE - elapsed);
            }
        }
    }

    fn schedule_wakeups(&mut self, ctx: &egui::Context, now: Instant) {
        if let Some(remaining) = self.scan_buffer.remaining_timeout(now) {
            ctx.request_repaint_after(remaining);
        }
        if let Some(until) = self.copied_until {
            if now < until {
                ctx.request_repaint_after(until - now);
            } else {
                self.copied_until = None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Footer & banners
    // -----------------------------------------------------------------------

    fn show_footer(&self, ctx: &egui::Context) {
        let text = footer_label(CURRENT_VERSION, self.config.active_printer());
        let issue_url = updater::issues_url(&self.config.github_repo);
        egui::TopBottomPanel::bottom("version_footer")
            .frame(
                Frame::new()
                    .fill(WINDOW_BG)
                    .inner_margin(Margin::symmetric(12, 7)),
            )
            .show_separator_line(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let btn_font = font_semibold(10.5);
                        let btn_w = text_button_width(ui, "Report an Issue", btn_font.clone(), 9.0);
                        let btn_style = ButtonStyle::solid(GRAY_150, GRAY_300, TEXT_BODY)
                            .with_stroke(GRAY_400)
                            .with_radius(10);
                        if flat_button(
                            ui,
                            egui::vec2(btn_w, 20.0),
                            "Report an Issue",
                            btn_font,
                            btn_style,
                        )
                        .on_hover_text(issue_url.as_str())
                        .clicked()
                        {
                            ctx.open_url(egui::OpenUrl::new_tab(issue_url.clone()));
                        }

                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(text)
                                        .font(font_regular(10.0))
                                        .color(FOOTER),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        });
                    });
                });
            });
    }

    fn update_banner_visible(&self) -> bool {
        if self.installing_update || self.update_phase == UpdatePhase::ReadyToRestart {
            return true;
        }
        self.available_update.is_some() && !self.update_banner_dismissed
    }

    fn show_banners(&mut self, ctx: &egui::Context) {
        let show_update = self.update_banner_visible();
        if !show_update
            && !self.installed_banner_visible
            && self.print_error.is_none()
            && self.misc_error.is_none()
        {
            return;
        }

        let mut actions: Vec<BannerAction> = Vec::new();
        egui::TopBottomPanel::top("top_banners")
            .frame(Frame::new().fill(WINDOW_BG).inner_margin(Margin {
                left: 0,
                right: 0,
                top: 10,
                bottom: 0,
            }))
            .show_separator_line(false)
            .resizable(false)
            .show(ctx, |ui| {
                centered_column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if show_update {
                        self.update_banner(ui, &mut actions);
                    }
                    if self.installed_banner_visible {
                        installed_banner(ui, &mut actions);
                    }
                    if let Some(message) = &self.print_error {
                        error_toast(ui, "Print failed", message, true, &mut actions, BannerAction::DismissPrintError);
                    }
                    if let Some(toast) = &self.misc_error {
                        error_toast(ui, toast.title, &toast.message, false, &mut actions, BannerAction::DismissMiscError);
                    }
                });
            });

        for action in actions {
            match action {
                BannerAction::InstallUpdate => self.start_update_install(),
                BannerAction::DismissUpdate => self.update_banner_dismissed = true,
                BannerAction::AddDesktopShortcut => match installer::create_desktop_shortcut() {
                    Ok(_) => {
                        self.shortcut_feedback =
                            Some((false, "Desktop shortcut created.".to_string()));
                        self.installed_banner_visible = false;
                    }
                    Err(err) => {
                        self.misc_error = Some(ErrorToast {
                            title: "Shortcut failed",
                            message: err.clone(),
                        });
                        self.shortcut_feedback = Some((true, err));
                    }
                },
                BannerAction::DismissInstalled => self.installed_banner_visible = false,
                BannerAction::OpenSettings => self.open_settings(),
                BannerAction::DismissPrintError => self.print_error = None,
                BannerAction::DismissMiscError => self.misc_error = None,
            }
        }
    }

    fn update_banner(&self, ui: &mut Ui, actions: &mut Vec<BannerAction>) {
        let version = self
            .available_update
            .as_ref()
            .map(|u| u.version.clone())
            .unwrap_or_default();

        Frame::new()
            .fill(UPDATE_BG)
            .stroke(Stroke::new(1.0_f32, UPDATE_BORDER))
            .corner_radius(8)
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let row_h = 28.0;
                ui.horizontal(|ui| {
                    ui.set_min_height(row_h);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match &self.update_phase {
                            UpdatePhase::Downloading { downloaded, total } => {
                                let fraction = total
                                    .filter(|t| *t > 0)
                                    .map(|t| (*downloaded as f32 / t as f32).clamp(0.0, 1.0));
                                let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
                                    .desired_width(110.0)
                                    .desired_height(10.0)
                                    .fill(NAVY);
                                ui.add(if fraction.is_some() { bar } else { bar.animate(true) });
                                banner_text(ui, &format!("Downloading update v{version}\u{2026}"));
                            }
                            UpdatePhase::Verifying => {
                                ui.add(egui::Spinner::new().size(14.0).color(NAVY));
                                banner_text(ui, "Verifying SHA-256 checksum\u{2026}");
                            }
                            UpdatePhase::ReadyToRestart => {
                                banner_text(ui, "Update installed \u{2014} restarting\u{2026}");
                            }
                            UpdatePhase::Error(message) if !self.installing_update => {
                                if close_icon_button(ui, 22.0, UPDATE_TEXT, UPDATE_BORDER)
                                    .on_hover_text("Dismiss")
                                    .clicked()
                                {
                                    actions.push(BannerAction::DismissUpdate);
                                }
                                if banner_button(ui, "Retry").clicked() {
                                    actions.push(BannerAction::InstallUpdate);
                                }
                                banner_text(ui, &format!("Update failed: {message}"))
                                    .on_hover_text(message.as_str());
                            }
                            _ => {
                                if close_icon_button(ui, 22.0, UPDATE_TEXT, UPDATE_BORDER)
                                    .on_hover_text("Later")
                                    .clicked()
                                {
                                    actions.push(BannerAction::DismissUpdate);
                                }
                                if banner_button(ui, "Update & Restart").clicked() {
                                    actions.push(BannerAction::InstallUpdate);
                                }
                                banner_text(ui, &format!("Update available: v{version}"));
                            }
                        }
                    });
                });
            });
    }

    // -----------------------------------------------------------------------
    // Homepage card
    // -----------------------------------------------------------------------

    fn show_home(&mut self, ctx: &egui::Context, now: Instant) {
        let mut actions: Vec<CardAction> = Vec::new();
        egui::CentralPanel::default()
            .frame(Frame::new().fill(WINDOW_BG))
            .show(ctx, |ui| {
                ScrollArea::vertical()
                    .id_salt("home_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(18.0);
                        centered_column(ui, |ui| self.card(ui, ctx, now, &mut actions));
                        ui.add_space(18.0);
                    });
            });

        for action in actions {
            match action {
                CardAction::OpenSettings => self.open_settings(),
                CardAction::Confirm(yes) => {
                    let effects = self.state.confirm_prompt(yes);
                    self.on_screen_changed(now);
                    self.execute_effects(effects, ctx, now);
                }
                CardAction::Override => {
                    let effects = self.state.trigger_override();
                    self.on_screen_changed(now);
                    self.execute_effects(effects, ctx, now);
                }
                CardAction::CopyAll => {
                    let effects = self.state.trigger_copy_all();
                    if !effects.is_empty() {
                        self.execute_effects(effects, ctx, now);
                        if self.last_clipboard_ok() {
                            self.copied_until = Some(now + COPIED_FEEDBACK);
                            ctx.request_repaint_after(COPIED_FEEDBACK);
                        }
                    }
                }
                CardAction::Reprint => {
                    let effects = self.state.trigger_reprint();
                    self.execute_effects(effects, ctx, now);
                }
                CardAction::Reset => self.reset_to_home(now),
            }
        }
    }

    fn card(&self, ui: &mut Ui, ctx: &egui::Context, now: Instant, actions: &mut Vec<CardAction>) {
        let card_frame = Frame::new()
            .fill(WHITE)
            .stroke(Stroke::new(1.0_f32, CARD_STROKE))
            .corner_radius(12)
            .shadow(Shadow {
                offset: [0, 4],
                blur: 14,
                spread: 0,
                color: Color32::from_black_alpha(22),
            })
            .inner_margin(Margin::symmetric(24, 26));

        let response = card_frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
            ui.vertical_centered(|ui| {
                self.brand_header(ui);
                status_pill(ui, self.is_generating(now));
                if self.config.active_printer().is_none() {
                    ui.add_space(10.0);
                    if printer_notice(ui) {
                        actions.push(CardAction::OpenSettings);
                    }
                }
                self.state_content(ui, ctx, now, actions);
                counter_instructions(ui, ctx);
            });
        });

        // Settings gear overlaid in the card's top-right corner.
        let card_rect = response.response.rect;
        let gear_rect = egui::Rect::from_center_size(
            egui::pos2(card_rect.right() - 24.0, card_rect.top() + 24.0),
            Vec2::splat(30.0),
        );
        let gear = ui
            .interact(gear_rect, ui.id().with("settings_gear"), Sense::CLICK)
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Settings");
        if ui.is_rect_visible(gear_rect) {
            let hovered = gear.hovered();
            let painter = ui.painter();
            if hovered {
                painter.circle_filled(gear_rect.center(), 15.0, GRAY_150);
            }
            paint_gear(
                painter,
                gear_rect.center(),
                9.5,
                if hovered { NAVY } else { MUTED },
                if hovered { GRAY_150 } else { WHITE },
            );
        }
        if gear.clicked() {
            actions.push(CardAction::OpenSettings);
        }
    }

    fn brand_header(&self, ui: &mut Ui) {
        let logo_font = font_bold_italic(28.0);
        let synthetic_italics = !self.fonts.true_bold_italic;
        let mut job = LayoutJob::default();
        for (text, color) in [("Postal", NAVY), ("Annex", BRAND_RED), ("+", NAVY)] {
            let mut format = text_format(logo_font.clone(), color, -1.0);
            format.italics = synthetic_italics;
            job.append(text, 0.0, format);
        }
        ui.add(Label::new(job).selectable(false));
        ui.add_space(5.0);

        let (rule, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), Sense::hover());
        ui.painter().rect_filled(rule, 0.0, NAVY);
        ui.add_space(6.0);

        let sub = LayoutJob::single_section(
            "AMAZON QR CODE SCANNER".to_owned(),
            text_format(font_semibold(11.0), MUTED, 2.0),
        );
        ui.add(Label::new(sub).selectable(false));
        ui.add_space(20.0);
    }

    fn state_content(
        &self,
        ui: &mut Ui,
        ctx: &egui::Context,
        now: Instant,
        actions: &mut Vec<CardAction>,
    ) {
        let screen = self.state.screen().clone();
        let is_ready = matches!(screen, ScreenState::Ready);
        let blink_visible = || {
            let (visible, until) = blink_phase(now.saturating_duration_since(self.screen_changed_at));
            ctx.request_repaint_after(until);
            visible
        };

        match screen {
            ScreenState::Ready => {}
            ScreenState::AwaitingPrompt(kind) => {
                let question = match kind {
                    PromptKind::HappyReturn { .. } => HAPPY_RETURN_QUESTION,
                    PromptKind::CustomerPacked { .. } => CUSTOMER_PACKED_QUESTION,
                };
                ui.add_space(16.0);
                if let Some(yes) = prompt_box(ui, question) {
                    actions.push(CardAction::Confirm(yes));
                }
            }
            ScreenState::HappyReturnNote => {
                ui.add_space(16.0);
                boxed(ui, NOTE_BG, Stroke::new(1.0_f32, YELLOW), 6, Margin::symmetric(12, 9), |ui| {
                    wrapped(ui, HAPPY_RETURN_NOTE, font_bold(13.0), NOTE_TEXT);
                });
            }
            ScreenState::PackingMaterialsNotice => {
                ui.add_space(16.0);
                let visible = blink_visible();
                boxed(ui, DANGER_BG, Stroke::new(2.0_f32, DANGER_BORDER), 8, Margin::same(18), |ui| {
                    blinking_title(ui, "NOTICE", RED, visible);
                    ui.add_space(4.0);
                    wrapped(ui, PACKING_NOTICE, font_bold(15.0), DANGER_TEXT);
                });
            }
            ScreenState::UnrecognizedWarning { .. } => {
                ui.add_space(16.0);
                let visible = blink_visible();
                boxed(ui, NOTE_BG, Stroke::new(2.0_f32, YELLOW), 8, Margin::same(18), |ui| {
                    blinking_title(ui, "WARNING!", AMBER_TEXT, visible);
                    ui.add_space(4.0);
                    wrapped(ui, UNRECOGNIZED_WARNING, font_bold(15.0), NOTE_TEXT);
                });
                ui.add_space(14.0);
                let style = ButtonStyle::solid(YELLOW, YELLOW_HOVER, TEXT_DARK).with_stroke(YELLOW_HOVER);
                if flat_button(
                    ui,
                    egui::vec2(ui.available_width(), 44.0),
                    "Force Print (Override)",
                    font_bold(14.0),
                    style,
                )
                .clicked()
                {
                    actions.push(CardAction::Override);
                }
            }
            ScreenState::LabelReady {
                payload,
                header,
                badge,
                ..
            } => {
                ui.add_space(16.0);
                if let Some(badge) = &badge {
                    alert_badge(ui, badge);
                    ui.add_space(18.0);
                } else {
                    ui.add_space(4.0);
                }
                if let Some(header) = header.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
                    let job = LayoutJob::single_section(
                        header.to_uppercase(),
                        text_format(font_bold(22.0), TEXT_DARK, 0.5),
                    );
                    ui.add(Label::new(job).wrap().selectable(false));
                    ui.add_space(12.0);
                }
                self.qr_preview(ui, ctx, now);
                ui.add_space(16.0);
                if self.raw_data_box(ui, &payload, now) {
                    actions.push(CardAction::CopyAll);
                }
                ui.add_space(14.0);
                if flat_button(
                    ui,
                    egui::vec2(ui.available_width(), 38.0),
                    "Reprint Last Label",
                    font_semibold(14.0),
                    secondary_button_style(),
                )
                .clicked()
                {
                    actions.push(CardAction::Reprint);
                }
            }
        }

        if !is_ready {
            ui.add_space(10.0);
            let reset_style = ButtonStyle::solid(WHITE, GRAY_150, TEXT_BODY).with_stroke(GRAY_400);
            if flat_button(
                ui,
                egui::vec2(ui.available_width(), 36.0),
                "Reset",
                font_semibold(14.0),
                reset_style,
            )
            .clicked()
            {
                actions.push(CardAction::Reset);
            }
        }
    }

    /// 300 x 300 QR preview with the block-assembly materialization animation (SPEC §6.2).
    fn qr_preview(&self, ui: &mut Ui, ctx: &egui::Context, now: Instant) {
        let size = ui.available_width().min(QR_PREVIEW_SIZE).floor();
        let (alloc, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let ppp = ctx.pixels_per_point();
        let min = egui::pos2(snap_to_pixel(alloc.min.x, ppp), snap_to_pixel(alloc.min.y, ppp));
        let rect = egui::Rect::from_min_size(min, Vec2::splat(size));

        if !ui.is_rect_visible(rect) {
            // Still keep the animation clock moving so it finishes promptly when scrolled back.
            if let Some(cached) = &self.cached_qr {
                if !cached.anim.is_finished_at(now) {
                    ctx.request_repaint();
                }
            }
            return;
        }

        ui.painter()
            .rect(rect, 0.0, WHITE, Stroke::new(1.0_f32, GRAY_200), StrokeKind::Outside);
        let painter = ui.painter_at(rect);

        let Some(cached) = &self.cached_qr else {
            if self.qr_error.is_some() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "QR preview unavailable",
                    font_semibold(13.0),
                    MUTED,
                );
            }
            return;
        };

        let width = cached.matrix.width;
        let total_modules = width + 2 * QUIET_ZONE_MODULES;
        let cell = size / total_modules as f32;
        let quiet = QUIET_ZONE_MODULES as f32;
        let elapsed = cached.anim.elapsed_secs(now);
        let mut mesh = Mesh::default();

        if !QrAnimation::is_finished_elapsed(elapsed) {
            let origin = rect.min + Vec2::splat(quiet * cell);
            for (row, col) in cached.matrix.dark_modules() {
                let st = animation::module_state(row, col, width, cached.anim.scan_nonce, elapsed);
                if st.alpha == 0 || st.scale <= 0.0 {
                    continue;
                }
                let center = origin
                    + Vec2::new(
                        (col as f32 + 0.5) * cell + st.offset_x_px,
                        (row as f32 + 0.5) * cell + st.offset_y_px,
                    );
                mesh.add_colored_rect(
                    egui::Rect::from_center_size(center, Vec2::splat(cell * st.scale)),
                    Color32::from_rgba_unmultiplied(0, 0, 0, st.alpha),
                );
            }
            ctx.request_repaint();
        } else {
            // Final state: pixel-snapped edges shared by neighbours, no anti-aliasing
            // feathering (plain mesh quads) => zero hairlines, pure #000000, no repaints.
            let edge_x = |i: usize| snap_to_pixel(rect.min.x + (i as f32 + quiet) * cell, ppp);
            let edge_y = |i: usize| snap_to_pixel(rect.min.y + (i as f32 + quiet) * cell, ppp);
            for (row, col) in cached.matrix.dark_modules() {
                mesh.add_colored_rect(
                    egui::Rect::from_min_max(
                        egui::pos2(edge_x(col), edge_y(row)),
                        egui::pos2(edge_x(col + 1), edge_y(row + 1)),
                    ),
                    Color32::BLACK,
                );
            }
        }
        painter.add(Shape::mesh(mesh));
    }

    /// Raw scanned data box with the overlaid "Copy All" button. Returns `true` on click.
    fn raw_data_box(&self, ui: &mut Ui, payload: &str, now: Instant) -> bool {
        let response = Frame::new()
            .fill(GRAY_200)
            .stroke(Stroke::new(1.0_f32, GRAY_400))
            .corner_radius(6)
            .inner_margin(Margin {
                left: 12,
                right: 84,
                top: 10,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(22.0);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ScrollArea::vertical()
                        .id_salt("raw_payload_scroll")
                        .max_height(58.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(payload).font(font_mono(12.0)).color(TEXT_BODY),
                                )
                                .wrap(),
                            );
                        });
                });
            });

        let box_rect = response.response.rect;
        let copied = self.copied_until.is_some_and(|until| now < until);
        let (label, style) = if copied {
            (
                "Copied!",
                ButtonStyle::solid(GREEN, GREEN, WHITE).with_stroke(GREEN).with_radius(4),
            )
        } else {
            (
                "Copy All",
                ButtonStyle::solid(WHITE, GRAY_300, TEXT_BODY)
                    .with_stroke(GRAY_400)
                    .with_radius(4),
            )
        };
        let btn_w = 68.0;
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(box_rect.right() - 8.0 - btn_w, box_rect.top() + 8.0),
            egui::vec2(btn_w, 24.0),
        );
        overlay_button(ui, btn_rect, ui.id().with("copy_all"), label, font_bold(11.5), style).clicked()
    }
}

const CLIPBOARD_TOAST_TITLE: &str = "Clipboard copy failed";

impl eframe::App for ScannerApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(WINDOW_BG).to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();

        self.poll_workers(ctx);

        if !self.is_modal_open() {
            Self::release_focus(ctx);
            self.capture_scan_input(ctx, now);
        }

        chrome::show(ctx);
        self.show_footer(ctx);
        self.show_banners(ctx);
        self.show_home(ctx, now);

        if self.settings_open {
            settings::show(self, ctx, now);
        }
        if self.whats_new_open {
            whats_new::show(self, ctx);
        }
        if !self.is_modal_open() {
            Self::release_focus(ctx);
        }

        self.flush_config_if_due(ctx, now);
        self.schedule_wakeups(ctx, now);
    }
}

// ---------------------------------------------------------------------------
// Stateless homepage pieces
// ---------------------------------------------------------------------------

fn initial_changelog_notes() -> String {
    updater::load_cached_changelog(CURRENT_VERSION)
        .unwrap_or_else(|| updater::CURRENT_CHANGELOG.to_string())
}

fn should_auto_open_whats_new(
    just_updated: bool,
    had_existing_config: bool,
    last_seen_version: Option<&str>,
    current_version: &str,
) -> bool {
    just_updated || (had_existing_config && last_seen_version != Some(current_version))
}

fn footer_label(version: &str, active_printer: Option<&str>) -> String {
    let printer = active_printer
        .map_or_else(|| "No Printer Configured".to_string(), |p| format!("Printer: {p}"));
    format!("QR Hub v{version}  \u{2022}  {printer}")
}

/// Lays out `add` in a horizontally centered column matching the card width.
fn centered_column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_width();
    let width = card_width(available);
    let side = ((available - width) * 0.5).max(0.0).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(side);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

/// Centered, wrapped, non-selectable label.
fn wrapped(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) {
    ui.add(
        Label::new(RichText::new(text).font(font).color(color))
            .wrap()
            .selectable(false),
    );
}

fn blinking_title(ui: &mut Ui, text: &str, color: Color32, visible: bool) {
    let color = if visible { color } else { Color32::TRANSPARENT };
    wrapped(ui, text, font_bold(18.0), color);
}

fn status_pill_label(is_generating: bool) -> &'static str {
    if is_generating {
        "Generating Code"
    } else {
        "Ready to Scan"
    }
}

/// Top scanner status pill — green "Ready to Scan" when idle, yellow "Generating Code" while scanning/animating.
fn status_pill(ui: &mut Ui, is_generating: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let (bg, border, dot, text_color, halo_inner, halo_outer) = if is_generating {
        (
            Color32::from_rgb(0xff, 0xf9, 0xe6),
            Color32::from_rgb(0xf5, 0xcf, 0x5c),
            YELLOW,
            NOTE_TEXT,
            0.38,
            0.20,
        )
    } else {
        (READY_BG, READY_BORDER, GREEN, NAVY, 0.30, 0.16)
    };
    painter.rect(rect, 8.0, bg, Stroke::new(1.0_f32, border), StrokeKind::Inside);

    let galley = painter.layout_no_wrap(
        status_pill_label(is_generating).to_owned(),
        font_bold(17.0),
        text_color,
    );
    let halo_r = 9.0;
    let gap = 10.0;
    let total = 2.0 * halo_r + gap + galley.size().x;
    let x0 = rect.center().x - total * 0.5;
    let dot_center = egui::pos2(x0 + halo_r, rect.center().y);
    painter.circle_filled(dot_center, halo_r, dot.gamma_multiply(halo_outer));
    painter.circle_filled(dot_center, 6.5, dot.gamma_multiply(halo_inner));
    painter.circle_filled(dot_center, 4.5, dot);
    let text_pos = egui::pos2(x0 + 2.0 * halo_r + gap, rect.center().y - galley.size().y * 0.5);
    painter.galley(text_pos, galley, text_color);
}

/// Amber "No thermal printer configured" notice. Returns `true` if "Open Settings" clicked.
fn printer_notice(ui: &mut Ui) -> bool {
    let mut clicked = false;
    boxed(ui, NOTE_BG, Stroke::new(1.0_f32, YELLOW), 8, Margin::symmetric(12, 8), |ui| {
        let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), Sense::hover());
        let font = font_semibold(12.5);
        let btn_w = text_button_width(ui, "Open Settings", font.clone(), 12.0);
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(row.right() - btn_w, row.top()),
            egui::vec2(btn_w, row.height()),
        );
        ui.painter().text(
            egui::pos2(row.left(), row.center().y),
            Align2::LEFT_CENTER,
            "No thermal printer configured",
            font_semibold(13.0),
            NOTE_TEXT,
        );
        let style = ButtonStyle::solid(WHITE, Color32::from_rgb(0xff, 0xe8, 0xa1), NOTE_TEXT)
            .with_stroke(YELLOW);
        clicked = overlay_button(ui, btn_rect, ui.id().with("printer_notice_settings"), "Open Settings", font, style)
            .clicked();
    });
    clicked
}

/// Shared Happy Return / Customer Packed prompt card. Returns `Some(yes)` when answered.
fn prompt_box(ui: &mut Ui, question: &str) -> Option<bool> {
    let mut answer = None;
    boxed(ui, GRAY_100, Stroke::new(2.0_f32, NAVY), 8, Margin::same(16), |ui| {
        wrapped(ui, question, font_bold(14.0), NAVY);
        ui.add_space(14.0);
        let gap = 12.0;
        let button_w = ((ui.available_width() - gap) * 0.5).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let size = egui::vec2(button_w, 42.0);
            if flat_button(ui, size, "No", font_bold(14.0), ButtonStyle::solid(RED, RED_HOVER, WHITE))
                .clicked()
            {
                answer = Some(false);
            }
            if flat_button(ui, size, "Yes", font_bold(14.0), ButtonStyle::solid(GREEN, GREEN_HOVER, WHITE))
                .clicked()
            {
                answer = Some(true);
            }
        });
    });
    answer
}

fn alert_badge(ui: &mut Ui, badge: &AlertBadge) {
    match badge {
        AlertBadge::Green { text } => {
            boxed(ui, GREEN, Stroke::NONE, 18, Margin::symmetric(20, 10), |ui| {
                wrapped(ui, text, font_bold(14.0), WHITE);
            });
        }
        AlertBadge::Yellow { text } => {
            boxed(ui, YELLOW, Stroke::NONE, 12, Margin::symmetric(16, 10), |ui| {
                wrapped(ui, text, font_bold(13.5), TEXT_DARK);
            });
        }
    }
}

fn counter_instructions(ui: &mut Ui, ctx: &egui::Context) {
    ui.add_space(26.0);
    let (rule, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    let ppp = ctx.pixels_per_point();
    let y = snap_to_pixel(rule.top(), ppp) + 0.5 / ppp;
    paint_dashed_rule(ui.painter(), rule.left(), rule.right(), y, GRAY_400);
    ui.add_space(14.0);

    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        let title = LayoutJob::single_section(
            "COUNTER INSTRUCTIONS".to_owned(),
            text_format(font_bold(10.0), MUTED, 1.0),
        );
        ui.add(Label::new(title).selectable(false));
        ui.add_space(6.0);
        for line in COUNTER_INSTRUCTIONS {
            ui.add(
                Label::new(RichText::new(line).font(font_regular(12.0)).color(TEXT_BODY))
                    .wrap()
                    .selectable(false),
            );
            ui.add_space(3.0);
        }
    });
}

fn installed_banner(ui: &mut Ui, actions: &mut Vec<BannerAction>) {
    let bg = Color32::from_rgb(0xe8, 0xf6, 0xec);
    let border = Color32::from_rgb(0xa3, 0xd9, 0xb1);
    let text_color = Color32::from_rgb(0x15, 0x57, 0x24);

    Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let row_h = 28.0;
            ui.horizontal(|ui| {
                ui.set_min_height(row_h);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_icon_button(ui, 22.0, text_color, border)
                        .on_hover_text("Dismiss")
                        .clicked()
                    {
                        actions.push(BannerAction::DismissInstalled);
                    }
                    let btn_font = font_bold(12.0);
                    let btn_w = text_button_width(ui, "Add Desktop Shortcut", btn_font.clone(), 12.0);
                    if flat_button(
                        ui,
                        egui::vec2(btn_w, 26.0),
                        "Add Desktop Shortcut",
                        btn_font,
                        ButtonStyle::solid(GREEN, GREEN_HOVER, WHITE),
                    )
                    .clicked()
                    {
                        actions.push(BannerAction::AddDesktopShortcut);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            Label::new(
                                RichText::new("Installed to Start Menu — launch anytime from Start.")
                                    .font(font_semibold(12.5))
                                    .color(text_color),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    });
                });
            });
        });
}

fn banner_text(ui: &mut Ui, text: &str) -> egui::Response {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add(
            Label::new(RichText::new(text).font(font_semibold(13.0)).color(UPDATE_TEXT))
                .truncate()
                .selectable(false),
        )
    })
    .inner
}

fn banner_button(ui: &mut Ui, label: &str) -> egui::Response {
    let font = font_bold(12.5);
    let width = text_button_width(ui, label, font.clone(), 12.0);
    flat_button(
        ui,
        egui::vec2(width, 28.0),
        label,
        font,
        ButtonStyle::solid(NAVY, NAVY_HOVER, WHITE),
    )
}

/// Dismissible red toast (`#f8d7da` / `#f5c6cb` / `#721c24`).
fn error_toast(
    ui: &mut Ui,
    title: &str,
    message: &str,
    offer_settings: bool,
    actions: &mut Vec<BannerAction>,
    dismiss: BannerAction,
) {
    Frame::new()
        .fill(DANGER_BG)
        .stroke(Stroke::new(1.0_f32, DANGER_BORDER))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.add(
                    Label::new(RichText::new(title).font(font_bold(13.5)).color(DANGER_TEXT))
                        .selectable(false),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_icon_button(ui, 22.0, DANGER_TEXT, DANGER_BORDER)
                        .on_hover_text("Dismiss")
                        .clicked()
                    {
                        actions.push(dismiss);
                    }
                    if offer_settings {
                        let font = font_semibold(12.0);
                        let width = text_button_width(ui, "Open Settings", font.clone(), 10.0);
                        let style = ButtonStyle::solid(WHITE, GRAY_100, DANGER_TEXT)
                            .with_stroke(DANGER_BORDER);
                        if flat_button(ui, egui::vec2(width, 24.0), "Open Settings", font, style)
                            .clicked()
                        {
                            actions.push(BannerAction::OpenSettings);
                        }
                    }
                });
            });
            ui.add(
                Label::new(RichText::new(message).font(font_regular(12.5)).color(DANGER_TEXT))
                    .wrap()
                    .selectable(false),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_instructions_match_spec_text() {
        assert_eq!(
            COUNTER_INSTRUCTIONS[0],
            "1. Scan QR code while app shows \"Ready to Scan\""
        );
        assert!(COUNTER_INSTRUCTIONS[1].contains("configured printer \u{2014} place on item"));
        assert!(COUNTER_INSTRUCTIONS[2].contains("CTRL + V in PostalMate"));
    }

    #[test]
    fn prompt_texts_match_spec() {
        assert!(HAPPY_RETURN_QUESTION.starts_with("This appears to be a Happy Return"));
        assert!(CUSTOMER_PACKED_QUESTION.starts_with("This item is marked as customer packed"));
        assert_eq!(HAPPY_RETURN_NOTE, "Please try this return in PostalMate first.");
    }

    #[test]
    fn status_pill_label_reflects_generating_state() {
        assert_eq!(status_pill_label(false), "Ready to Scan");
        assert_eq!(status_pill_label(true), "Generating Code");
    }

    #[test]
    fn whats_new_auto_opens_only_after_update_or_version_bump_on_existing_install() {
        // Fresh install without --just-updated should not nag.
        assert!(!should_auto_open_whats_new(false, false, None, "2.0.2"));
        // Explicit --just-updated always opens.
        assert!(should_auto_open_whats_new(true, false, None, "2.0.2"));
        assert!(should_auto_open_whats_new(true, true, Some("2.0.2"), "2.0.2"));
        // Existing install upgraded from previous/untracked version opens once.
        assert!(should_auto_open_whats_new(false, true, None, "2.0.2"));
        assert!(should_auto_open_whats_new(false, true, Some("2.0.1"), "2.0.2"));
        // Normal subsequent launch on same version does not reopen.
        assert!(!should_auto_open_whats_new(false, true, Some("2.0.2"), "2.0.2"));
    }

    #[test]
    fn footer_label_uses_qr_hub_branding() {
        assert_eq!(
            footer_label("2.0.2", None),
            "QR Hub v2.0.2  \u{2022}  No Printer Configured"
        );
        assert_eq!(
            footer_label("2.0.2", Some("Zebra ZD421")),
            "QR Hub v2.0.2  \u{2022}  Printer: Zebra ZD421"
        );
    }
}
