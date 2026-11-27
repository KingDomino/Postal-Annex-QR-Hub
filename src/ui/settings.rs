//! Settings modal overlay: thermal printer selection, 4" x 6" calibration, and updates.

use std::time::Instant;

use eframe::egui::{
    self, Align, Color32, ComboBox, DragValue, Id, Layout, Modal, ScrollArea, Slider, TextEdit, Ui,
};

use postal_annex_qr_scanner::config::{AppConfig, UpdateChannel};
use postal_annex_qr_scanner::installer;
use postal_annex_qr_scanner::updater::{current_arch_label, CURRENT_VERSION};

use super::app::{ScannerApp, TestPrintStatus, UpdatePhase};
use super::colors::*;
use super::{
    close_icon_button, download_progress_bar, flat_button, font_bold, font_regular, font_semibold,
    format_bytes, label, modal_frame, secondary_button_style, spinner_row, text_button_width,
    ButtonStyle, MODAL_WIDTH,
};

/// Renders the centered Settings modal over a click-blocking backdrop scrim.
pub(super) fn show(app: &mut ScannerApp, ctx: &egui::Context, now: Instant) {
    let mut should_close = false;
    let mut open_whats_new = false;
    let mut config_changed_discrete = false;
    let mut config_changed_continuous = false;
    let shift_held = ctx.input(|i| i.modifiers.shift);
    app.show_channel_picker.get_or_insert(shift_held);

    let max_body_height = (ctx.screen_rect().height() - 150.0).clamp(180.0, 520.0);

    let modal_response = Modal::new(Id::new("settings_modal"))
        .backdrop_color(Color32::from_black_alpha(110))
        .frame(modal_frame())
        .show(ctx, |ui| {
            ui.set_width(MODAL_WIDTH - 40.0);
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            // Header bar
            ui.horizontal(|ui| {
                ui.add(label("Settings", font_bold(18.0), NAVY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_icon_button(ui, 26.0, MUTED, GRAY_150)
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        should_close = true;
                    }
                });
            });

            ui.add(egui::Separator::default().spacing(8.0));

            ScrollArea::vertical()
                .id_salt("settings_scroll")
                .max_height(max_body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

                    // Section 1: Thermal Printer
                    printer_section(app, ui, &mut config_changed_discrete);

                    ui.add(egui::Separator::default().spacing(12.0));

                    // Section 2: 4" x 6" Label Calibration
                    calibration_section(
                        app,
                        ui,
                        &mut config_changed_discrete,
                        &mut config_changed_continuous,
                    );

                    ui.add(egui::Separator::default().spacing(12.0));

                    // Section 3: Updates & Info
                    updates_section(
                        app,
                        ui,
                        &mut config_changed_discrete,
                        &mut config_changed_continuous,
                        &mut open_whats_new,
                    );

                    ui.add_space(4.0);
                });

            ui.add(egui::Separator::default().spacing(8.0));

            // Footer bar
            ui.horizontal(|ui| {
                if let Some(err) = &app.config_error {
                    ui.add(
                        label(
                            format!("Save error: {err}"),
                            font_regular(11.0),
                            DANGER_TEXT,
                        )
                        .truncate(),
                    )
                    .on_hover_text(err.as_str());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let font = font_bold(13.5);
                    let w = text_button_width(ui, "Done", font.clone(), 18.0);
                    if flat_button(
                        ui,
                        egui::vec2(w, 32.0),
                        "Done",
                        font,
                        ButtonStyle::solid(NAVY, NAVY_HOVER, WHITE),
                    )
                    .clicked()
                    {
                        should_close = true;
                    }
                });
            });
        });

    if config_changed_discrete {
        app.config_dirty_at = Some(now);
        app.save_config_now();
    } else if config_changed_continuous {
        app.config_dirty_at = Some(now);
    }

    if open_whats_new {
        app.close_settings();
        app.open_whats_new();
    } else if should_close || modal_response.should_close() {
        app.close_settings();
    }
}

fn section_heading(ui: &mut Ui, title: &str) {
    ui.add(label(title, font_bold(12.0), NAVY));
}

fn printer_section(app: &mut ScannerApp, ui: &mut Ui, changed: &mut bool) {
    section_heading(ui, "THERMAL PRINTER");

    let current_name = app.config.active_printer().map(str::to_owned);
    let current_offline = current_name
        .as_deref()
        .is_some_and(|name| !app.printers_refreshing && !app.printers.iter().any(|p| p == name));

    let selected_label = match &current_name {
        Some(name) if current_offline => format!("{name} (offline)"),
        Some(name) => name.clone(),
        None => "Select a printer\u{2026}".to_string(),
    };

    ui.horizontal(|ui| {
        let refresh_font = font_semibold(12.5);
        let refresh_label = if app.printers_refreshing {
            "Refreshing\u{2026}"
        } else {
            "Refresh List"
        };
        let refresh_w = text_button_width(ui, "Refresh List", refresh_font.clone(), 10.0).max(94.0);
        let combo_w = (ui.available_width() - refresh_w - 8.0).max(140.0);

        ComboBox::from_id_salt("printer_picker")
            .width(combo_w)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                let is_none = app.config.active_printer().is_none();
                if ui
                    .selectable_label(is_none, "(No printer selected)")
                    .clicked()
                {
                    app.config.printer_name = None;
                    *changed = true;
                }
                if let Some(name) = &current_name {
                    if current_offline {
                        let offline_label = format!("{name} (offline)");
                        if ui.selectable_label(true, offline_label).clicked() {
                            app.config.printer_name = Some(name.clone());
                        }
                    }
                }
                for printer in &app.printers {
                    let selected = current_name.as_deref() == Some(printer.as_str());
                    if ui.selectable_label(selected, printer).clicked() {
                        app.config.printer_name = Some(printer.clone());
                        *changed = true;
                    }
                }
            });

        let style = secondary_button_style();
        if flat_button(
            ui,
            egui::vec2(refresh_w, 26.0),
            refresh_label,
            refresh_font,
            style,
        )
        .clicked()
            && !app.printers_refreshing
        {
            app.request_printer_refresh();
        }
    });

    if app.printers_refreshing {
        spinner_row(ui, "Scanning installed printers\u{2026}", MUTED);
    } else if let Some(err) = &app.printers_error {
        ui.add(label(err, font_regular(12.0), DANGER_TEXT).wrap());
    }

    if ui
        .checkbox(&mut app.config.auto_print_on_scan, "Auto-print on scan")
        .changed()
    {
        *changed = true;
    }

    let test_sending = app.test_print == TestPrintStatus::Sending;
    let test_label = if test_sending {
        "Sending Test Label\u{2026}"
    } else {
        "Test Print 4\" x 6\" Label"
    };
    let test_style = ButtonStyle::solid(GRAY_150, GRAY_300, NAVY).with_stroke(READY_BORDER);
    if flat_button(
        ui,
        egui::vec2(ui.available_width(), 32.0),
        test_label,
        font_semibold(13.0),
        test_style,
    )
    .clicked()
        && !test_sending
    {
        app.send_test_print();
    }

    match &app.test_print {
        TestPrintStatus::Idle => {}
        TestPrintStatus::Sending => {
            spinner_row(ui, "Sending 4\" x 6\" test label to spooler\u{2026}", MUTED);
        }
        TestPrintStatus::Sent => {
            ui.add(label("Test label sent to printer.", font_semibold(12.0), GREEN).wrap());
        }
        TestPrintStatus::Failed(msg) => {
            ui.add(
                label(
                    format!("Test print failed: {msg}"),
                    font_regular(12.0),
                    DANGER_TEXT,
                )
                .wrap(),
            );
        }
    }
}

fn calibration_section(
    app: &mut ScannerApp,
    ui: &mut Ui,
    changed_discrete: &mut bool,
    changed_continuous: &mut bool,
) {
    section_heading(ui, "4\" \u{00d7} 6\" LABEL CALIBRATION");

    if ui
        .checkbox(
            &mut app.config.force_4x6_devmode,
            "Force 4\" x 6\" DEVMODEW",
        )
        .on_hover_text("Overrides the printer driver's default paper size to 4\" x 6\" portrait.")
        .changed()
    {
        *changed_discrete = true;
    }

    ui.horizontal(|ui| {
        ui.add(label("QR Size:", font_regular(13.0), TEXT_BODY));
        let slider = Slider::new(&mut app.config.qr_size_inches, 2.0..=3.5)
            .step_by(0.025)
            .fixed_decimals(3)
            .suffix("\"");
        if ui.add(slider).changed() {
            *changed_continuous = true;
        }
    });

    ui.horizontal(|ui| {
        ui.add(label("Offset X:", font_regular(13.0), TEXT_BODY));
        if ui
            .add(
                DragValue::new(&mut app.config.offset_x_mm)
                    .range(-20.0..=20.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            )
            .changed()
        {
            *changed_continuous = true;
        }

        ui.add_space(14.0);

        ui.add(label("Offset Y:", font_regular(13.0), TEXT_BODY));
        if ui
            .add(
                DragValue::new(&mut app.config.offset_y_mm)
                    .range(-20.0..=20.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            )
            .changed()
        {
            *changed_continuous = true;
        }
    });

    let reset_font = font_semibold(12.5);
    let reset_w = text_button_width(ui, "Reset Calibration", reset_font.clone(), 12.0);
    if flat_button(
        ui,
        egui::vec2(reset_w, 28.0),
        "Reset Calibration",
        reset_font,
        secondary_button_style(),
    )
    .clicked()
    {
        reset_calibration(&mut app.config);
        *changed_discrete = true;
    }
}

fn updates_section(
    app: &mut ScannerApp,
    ui: &mut Ui,
    changed_discrete: &mut bool,
    changed_continuous: &mut bool,
    open_whats_new: &mut bool,
) {
    section_heading(ui, "UPDATES & INFO");

    ui.horizontal(|ui| {
        ui.add(label(
            format!(
                "Version: v{CURRENT_VERSION}   \u{2022}   Architecture: {}",
                current_arch_label()
            ),
            font_regular(12.5),
            MUTED,
        ));
    });

    if ui
        .checkbox(
            &mut app.config.auto_check_updates,
            "Check for updates on startup",
        )
        .changed()
    {
        *changed_discrete = true;
    }

    // Hidden unless Settings was opened with Shift held (or this PC is already on Test,
    // so it can always be switched back).
    if app.show_channel_picker == Some(true) || app.config.update_channel == UpdateChannel::Test {
        let before = app.config.update_channel;
        ui.horizontal(|ui| {
            ui.add(label("Update channel:", font_regular(13.0), TEXT_BODY));
            egui::ComboBox::from_id_salt("update_channel")
                .selected_text(match app.config.update_channel {
                    UpdateChannel::Stable => "Stable",
                    UpdateChannel::Test => "Test (pre-releases)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut app.config.update_channel,
                        UpdateChannel::Stable,
                        "Stable",
                    );
                    ui.selectable_value(
                        &mut app.config.update_channel,
                        UpdateChannel::Test,
                        "Test (pre-releases)",
                    );
                });
        });
        if app.config.update_channel != before {
            *changed_discrete = true;
            if !app.installing_update {
                app.check_for_updates();
            }
        }
    }

    ui.horizontal(|ui| {
        ui.add(label("GitHub Repo:", font_regular(13.0), TEXT_BODY));
        let edit = TextEdit::singleline(&mut app.config.github_repo)
            .desired_width(ui.available_width())
            .hint_text("owner/repo");
        if ui.add(edit).changed() {
            *changed_continuous = true;
        }
    });

    let busy = matches!(
        app.update_phase,
        UpdatePhase::Checking
            | UpdatePhase::Downloading { .. }
            | UpdatePhase::Verifying
            | UpdatePhase::ReadyToRestart
    );

    ui.horizontal(|ui| {
        let btn_font = font_semibold(12.5);
        let check_label = if app.update_phase == UpdatePhase::Checking {
            "Checking\u{2026}"
        } else {
            "Check for Updates"
        };
        let check_w = text_button_width(ui, "Check for Updates", btn_font.clone(), 12.0);
        if flat_button(
            ui,
            egui::vec2(check_w, 30.0),
            check_label,
            btn_font.clone(),
            secondary_button_style(),
        )
        .clicked()
            && !busy
        {
            app.check_for_updates();
        }

        let whats_new_w = text_button_width(ui, "What's New?", btn_font.clone(), 12.0);
        if flat_button(
            ui,
            egui::vec2(whats_new_w, 30.0),
            "What's New?",
            btn_font,
            secondary_button_style(),
        )
        .clicked()
        {
            *open_whats_new = true;
        }
    });

    if app.available_update.is_some()
        && !app.installing_update
        && app.update_phase != UpdatePhase::ReadyToRestart
    {
        let install_font = font_bold(12.5);
        let install_w = text_button_width(ui, "Update & Restart", install_font.clone(), 14.0);
        if flat_button(
            ui,
            egui::vec2(install_w, 30.0),
            "Update & Restart",
            install_font,
            ButtonStyle::solid(NAVY, NAVY_HOVER, WHITE),
        )
        .clicked()
        {
            app.start_update_install();
        }
    }

    match &app.update_phase {
        UpdatePhase::Idle => {}
        UpdatePhase::Checking => {
            spinner_row(ui, "Checking GitHub Releases\u{2026}", MUTED);
        }
        UpdatePhase::UpToDate => {
            ui.add(label("Up to date.", font_semibold(12.0), GREEN));
        }
        UpdatePhase::Available => {
            if let Some(update) = &app.available_update {
                ui.add(
                    label(
                        format!(
                            "Update available: v{} ({})",
                            update.version, update.asset_name
                        ),
                        font_semibold(12.5),
                        NAVY,
                    )
                    .wrap(),
                );
            }
        }
        UpdatePhase::Downloading { downloaded, total } => {
            let progress_text = match total {
                Some(t) if *t > 0 => format!(
                    "Downloading: {} / {}",
                    format_bytes(*downloaded),
                    format_bytes(*t)
                ),
                _ => format!("Downloading: {}", format_bytes(*downloaded)),
            };
            ui.add(label(progress_text, font_regular(12.0), UPDATE_TEXT));
            ui.add(download_progress_bar(
                *downloaded,
                *total,
                ui.available_width(),
            ));
        }
        UpdatePhase::Verifying => {
            spinner_row(ui, "Verifying SHA-256 checksum\u{2026}", UPDATE_TEXT);
        }
        UpdatePhase::ReadyToRestart => {
            ui.add(label(
                "Update installed \u{2014} restarting\u{2026}",
                font_semibold(12.0),
                GREEN,
            ));
        }
        UpdatePhase::Error(message) | UpdatePhase::InstallFailed(message) => {
            ui.add(label(message, font_regular(12.0), DANGER_TEXT).wrap());
        }
    }

    ui.add_space(4.0);
    if cfg!(windows) {
        let location_text = installer::install_dir()
            .map(|p| format!("Install folder: {}", p.display()))
            .unwrap_or_else(|| "Install folder: %LOCALAPPDATA%\\QR Hub".to_string());
        ui.add(label(location_text, font_regular(11.0), MUTED).truncate());
    }

    ui.horizontal(|ui| {
        let btn_font = font_semibold(12.0);
        let btn_style = secondary_button_style();

        let desktop_w = text_button_width(ui, "Add Desktop Shortcut", btn_font.clone(), 10.0);
        if flat_button(
            ui,
            egui::vec2(desktop_w, 28.0),
            "Add Desktop Shortcut",
            btn_font.clone(),
            btn_style,
        )
        .clicked()
        {
            let _ = app.add_desktop_shortcut();
        }

        let start_w = text_button_width(ui, "Repair Start Menu", btn_font.clone(), 10.0);
        if flat_button(
            ui,
            egui::vec2(start_w, 28.0),
            "Repair Start Menu",
            btn_font,
            btn_style,
        )
        .clicked()
        {
            let _ = app.record_shortcut_result(
                installer::ensure_start_menu_shortcut(),
                "Start Menu shortcut updated.",
            );
        }
    });

    if let Some((is_err, msg)) = &app.shortcut_feedback {
        let color = if *is_err { DANGER_TEXT } else { GREEN };
        ui.add(label(msg, font_semibold(12.0), color).wrap());
    }
}

/// Resets the 4" x 6" label calibration parameters to factory defaults.
pub(crate) fn reset_calibration(config: &mut AppConfig) {
    let defaults = AppConfig::default();
    config.force_4x6_devmode = defaults.force_4x6_devmode;
    config.qr_size_inches = defaults.qr_size_inches;
    config.offset_x_mm = defaults.offset_x_mm;
    config.offset_y_mm = defaults.offset_y_mm;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_calibration_restores_defaults_without_clobbering_printer_or_repo() {
        let mut cfg = AppConfig {
            printer_name: Some("ZDesigner GK420d".to_string()),
            auto_print_on_scan: false,
            force_4x6_devmode: false,
            qr_size_inches: 2.5,
            offset_x_mm: 3.2,
            offset_y_mm: -4.5,
            github_repo: "custom/repo".to_string(),
            ..AppConfig::default()
        };

        reset_calibration(&mut cfg);

        assert!(cfg.force_4x6_devmode);
        assert_eq!(cfg.qr_size_inches, 3.125);
        assert_eq!(cfg.offset_x_mm, 0.0);
        assert_eq!(cfg.offset_y_mm, 0.0);
        // Unrelated settings preserved
        assert_eq!(cfg.printer_name.as_deref(), Some("ZDesigner GK420d"));
        assert!(!cfg.auto_print_on_scan);
        assert_eq!(cfg.github_repo, "custom/repo");
    }
}
