#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use std::sync::Arc;

use eframe::egui;
use postal_annex_qr_scanner::icon::window_icon_rgba;
use postal_annex_qr_scanner::installer::{self, StartupAction};

const WINDOW_TITLE: &str = "QR Hub";

fn main() -> eframe::Result<()> {
    let (just_installed, just_updated) = match installer::handle_startup() {
        StartupAction::RunApp {
            just_installed,
            just_updated,
        } => (just_installed, just_updated),
        StartupAction::Exit => return Ok(()),
    };

    let (rgba, width, height) = window_icon_rgba();
    let icon_data = egui::IconData {
        rgba,
        width,
        height,
    };

    let native_options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_title(WINDOW_TITLE)
            .with_inner_size([460.0, 740.0])
            .with_min_inner_size([400.0, 560.0])
            .with_resizable(true)
            .with_icon(Arc::new(icon_data)),
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        native_options,
        Box::new(move |cc| Ok(Box::new(ui::ScannerApp::new(cc, just_installed, just_updated)))),
    )
}
