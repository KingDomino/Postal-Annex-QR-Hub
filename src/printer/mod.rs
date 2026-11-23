//! Direct thermal label layout math, platform printer abstraction, and background print worker.

#[cfg(windows)]
pub mod win32;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::config::AppConfig;
use crate::domain::{PrintJob, PrintTrigger};
use crate::qr::QUIET_ZONE_MODULES;

/// Physical printer device metrics retrieved from the spooler DC (or simulated on non-Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMetrics {
    pub dpi_x: i32,
    pub dpi_y: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub physical_offset_x: i32,
    pub physical_offset_y: i32,
}

/// Exact DPI-aligned integer layout of a 4" x 6" thermal label in printable-area device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelLayout {
    /// Exact integer pixels per QR module (>= 1, never fractional).
    pub module_px: i32,
    /// Actual printed QR square size (`module_px * matrix_width`).
    pub qr_actual_px: i32,
    /// Vertical gap between header and QR code (`round(0.21 * dpi_y)` when header is present).
    pub gap_px: i32,
    /// Effective header height in device pixels (`0` when no header).
    pub header_height_px: i32,
    /// Top Y coordinate of the header in printable-area device pixels.
    pub header_y_px: i32,
    /// Horizontal center X coordinate of the header/label in printable-area device pixels.
    pub header_center_x_px: i32,
    /// Left X coordinate of the QR matrix in printable-area device pixels.
    pub qr_origin_x_px: i32,
    /// Top Y coordinate of the QR matrix in printable-area device pixels.
    pub qr_origin_y_px: i32,
    /// Total height of the `[Header + Gap + QR]` block in device pixels.
    pub total_block_height_px: i32,
}

/// Computes the exact integer-snapped layout for a thermal label.
///
/// Guarantees:
/// - `module_px = max(1, floor(round(qr_size_inches * dpi_x) / N))`, clamped so a
///   4-module quiet zone fits cleanly within `physical_width` when `physical_width > 0`.
/// - Centers `[Header + Gap + QR]` across the physical label, compensating for
///   `PHYSICALOFFSETX` / `PHYSICALOFFSETY` and applying `offset_x_mm` / `offset_y_mm`.
pub fn compute_label_layout(
    metrics: DeviceMetrics,
    matrix_width: usize,
    has_header: bool,
    header_height_px: i32,
    config: &AppConfig,
) -> LabelLayout {
    let n = matrix_width.max(1) as i32;
    let dpi_x = metrics.dpi_x.max(72);
    let dpi_y = metrics.dpi_y.max(72);

    let physical_width = if metrics.physical_width > 0 {
        metrics.physical_width
    } else {
        (config.label_width_inches.max(1.0) * dpi_x as f32).round() as i32
    };
    let physical_height = if metrics.physical_height > 0 {
        metrics.physical_height
    } else {
        (config.label_height_inches.max(1.0) * dpi_y as f32).round() as i32
    };

    let target_qr_px = (config.qr_size_inches.clamp(0.5, config.label_width_inches.max(1.0))
        * dpi_x as f32)
        .round() as i32;

    let mut module_px = (target_qr_px / n).max(1);
    if metrics.physical_width > 0 {
        let total_modules_with_quiet = n + (QUIET_ZONE_MODULES as i32) * 2;
        let max_fit_module_px = (metrics.physical_width / total_modules_with_quiet).max(1);
        module_px = module_px.min(max_fit_module_px).max(1);
    }

    let qr_actual_px = module_px * n;

    let (eff_header_h, gap_px) = if has_header {
        (
            header_height_px.max(0),
            (0.21_f32 * dpi_y as f32).round() as i32,
        )
    } else {
        (0, 0)
    };

    let total_block_height_px = eff_header_h + gap_px + qr_actual_px;

    let offset_x_px = ((config.offset_x_mm / 25.4) * dpi_x as f32).round() as i32;
    let offset_y_px = ((config.offset_y_mm / 25.4) * dpi_y as f32).round() as i32;

    let header_center_x_px = (physical_width / 2) - metrics.physical_offset_x + offset_x_px;
    let block_top_y =
        ((physical_height - total_block_height_px) / 2) - metrics.physical_offset_y + offset_y_px;

    let qr_origin_x_px = header_center_x_px - (qr_actual_px / 2);
    let header_y_px = block_top_y;
    let qr_origin_y_px = if has_header {
        block_top_y + eff_header_h + gap_px
    } else {
        block_top_y
    };

    LabelLayout {
        module_px,
        qr_actual_px,
        gap_px,
        header_height_px: eff_header_h,
        header_y_px,
        header_center_x_px,
        qr_origin_x_px,
        qr_origin_y_px,
        total_block_height_px,
    }
}

/// Commands dispatched to the background [`PrintWorker`] thread.
#[derive(Debug, Clone)]
pub enum PrintCommand {
    Print { job: PrintJob, config: AppConfig },
    RefreshPrinters,
}

/// Events emitted by the background [`PrintWorker`] thread for the UI to consume.
#[derive(Debug, Clone)]
pub enum PrintEvent {
    PrintSucceeded { header: Option<String> },
    PrintFailed(String),
    PrintersLoaded(Result<Vec<String>, String>),
}

/// Dedicated background printing and printer-enumeration worker thread.
pub struct PrintWorker {
    tx: Sender<PrintCommand>,
    rx: Receiver<PrintEvent>,
}

impl PrintWorker {
    /// Spawns the background printer worker thread.
    ///
    /// Pass an optional `repaint_cb` (e.g. `Arc::new(move || ctx.request_repaint())`) so
    /// the UI wakes immediately when print jobs finish or printer lists arrive.
    pub fn spawn(repaint_cb: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<PrintCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<PrintEvent>();

        thread::Builder::new()
            .name("printer-worker".to_string())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    let event = match cmd {
                        PrintCommand::Print { job, config } => match print_label(&job, &config) {
                            Ok(()) => PrintEvent::PrintSucceeded { header: job.header },
                            Err(err) => PrintEvent::PrintFailed(err),
                        },
                        PrintCommand::RefreshPrinters => {
                            PrintEvent::PrintersLoaded(list_printers())
                        }
                    };

                    if evt_tx.send(event).is_err() {
                        break;
                    }
                    if let Some(cb) = &repaint_cb {
                        cb();
                    }
                }
            })
            .expect("failed to spawn printer-worker thread");

        Self {
            tx: cmd_tx,
            rx: evt_rx,
        }
    }

    /// Unconditionally queues a print job on the background thread.
    pub fn print(&self, job: PrintJob, config: AppConfig) {
        let _ = self.tx.send(PrintCommand::Print { job, config });
    }

    /// Queues a print job respecting [`PrintTrigger`] and `config.auto_print_on_scan`.
    ///
    /// - [`PrintTrigger::Auto`]: prints only if `config.auto_print_on_scan` is `true`.
    /// - [`PrintTrigger::Manual`]: always prints (Override, Reprint, Test Print).
    pub fn print_with_trigger(&self, job: PrintJob, trigger: PrintTrigger, config: AppConfig) {
        if trigger == PrintTrigger::Auto && !config.auto_print_on_scan {
            return;
        }
        self.print(job, config);
    }

    /// Requests asynchronous enumeration of installed printers.
    pub fn refresh_printers(&self) {
        let _ = self.tx.send(PrintCommand::RefreshPrinters);
    }

    /// Non-blocking poll for the next completed [`PrintEvent`].
    pub fn try_recv(&self) -> Option<PrintEvent> {
        self.rx.try_recv().ok()
    }
}

/// Enumerates available printers on the current platform.
pub fn list_printers() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        win32::enumerate_printers()
    }
    #[cfg(not(windows))]
    {
        Ok(vec!["Simulated 4x6 Thermal Printer (Preview)".to_string()])
    }
}

/// Prints a single thermal label synchronously.
pub fn print_label(job: &PrintJob, config: &AppConfig) -> Result<(), String> {
    #[cfg(windows)]
    {
        win32::print_label_gdi(job, config)
    }
    #[cfg(not(windows))]
    {
        let _printer_name = config.active_printer().ok_or_else(|| {
            "No printer configured. Open Settings to select a 4\" x 6\" thermal printer."
                .to_string()
        })?;
        let matrix = crate::qr::encode(&job.payload)?;
        let simulated_metrics = DeviceMetrics {
            dpi_x: 203,
            dpi_y: 203,
            physical_width: 812,
            physical_height: 1218,
            physical_offset_x: 0,
            physical_offset_y: 0,
        };
        let has_header = job
            .header
            .as_deref()
            .map(str::trim)
            .is_some_and(|h| !h.is_empty());
        let header_height_px = if has_header { 73 } else { 0 };
        let _layout = compute_label_layout(
            simulated_metrics,
            matrix.width,
            has_header,
            header_height_px,
            config,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_203_dpi_centered_with_header_and_quiet_zone() {
        let cfg = AppConfig::default(); // 4x6, qr=3.125"
        let metrics = DeviceMetrics {
            dpi_x: 203,
            dpi_y: 203,
            physical_width: 812,
            physical_height: 1218,
            physical_offset_x: 12,
            physical_offset_y: 16,
        };

        let layout = compute_label_layout(metrics, 33, true, 73, &cfg);
        // round(3.125 * 203) = 634; floor(634 / 33) = 19; quiet zone check: 812 / (33 + 8) = 19
        assert_eq!(layout.module_px, 19);
        assert_eq!(layout.qr_actual_px, 19 * 33);
        assert_eq!(layout.gap_px, (0.21_f32 * 203.0).round() as i32);
        assert_eq!(layout.header_height_px, 73);
        assert_eq!(
            layout.total_block_height_px,
            73 + layout.gap_px + layout.qr_actual_px
        );
        // Centered horizontally compensating for physical_offset_x = 12
        assert_eq!(layout.header_center_x_px, 406 - 12);
        assert_eq!(
            layout.qr_origin_x_px,
            layout.header_center_x_px - (layout.qr_actual_px / 2)
        );
        // QR lies below header + gap
        assert_eq!(layout.qr_origin_y_px, layout.header_y_px + 73 + layout.gap_px);
    }

    #[test]
    fn layout_headerless_override_centers_qr_only_and_applies_mm_offsets() {
        let cfg = AppConfig {
            offset_x_mm: 2.54, // +0.1 in -> +30 px at 300 DPI
            offset_y_mm: -2.54, // -0.1 in -> -30 px at 300 DPI
            ..Default::default()
        };
        let metrics = DeviceMetrics {
            dpi_x: 300,
            dpi_y: 300,
            physical_width: 1200,
            physical_height: 1800,
            physical_offset_x: 0,
            physical_offset_y: 0,
        };

        let layout = compute_label_layout(metrics, 29, false, 100, &cfg);
        assert_eq!(layout.header_height_px, 0);
        assert_eq!(layout.gap_px, 0);
        assert_eq!(layout.total_block_height_px, layout.qr_actual_px);
        assert_eq!(layout.header_center_x_px, 600 + 30);
        assert_eq!(
            layout.qr_origin_y_px,
            (1800 - layout.qr_actual_px) / 2 - 30
        );
    }

    #[test]
    fn layout_clamps_module_px_to_preserve_4_module_quiet_zone() {
        let cfg = AppConfig {
            qr_size_inches: 3.9, // intentionally large
            ..Default::default()
        };
        let metrics = DeviceMetrics {
            dpi_x: 203,
            dpi_y: 203,
            physical_width: 812,
            physical_height: 1218,
            physical_offset_x: 0,
            physical_offset_y: 0,
        };
        let n = 29;
        let layout = compute_label_layout(metrics, n, false, 0, &cfg);
        // Must leave at least 4 modules of quiet zone on both left & right
        let required_with_quiet = layout.module_px * (n as i32 + 8);
        assert!(required_with_quiet <= metrics.physical_width);
    }

    #[cfg(not(windows))]
    #[test]
    fn worker_respects_auto_print_setting_and_enumerates_simulated_printer() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        let repaints = Arc::new(AtomicUsize::new(0));
        let repaints_cb = Arc::clone(&repaints);
        let worker = PrintWorker::spawn(Some(Arc::new(move || {
            repaints_cb.fetch_add(1, Ordering::SeqCst);
        })));

        // Auto print skipped when auto_print_on_scan is false
        let disabled_cfg = AppConfig {
            printer_name: Some("Simulated 4x6 Thermal Printer (Preview)".to_string()),
            auto_print_on_scan: false,
            ..Default::default()
        };
        let job = crate::domain::ScannerStateMachine::test_print_job();
        worker.print_with_trigger(job.clone(), PrintTrigger::Auto, disabled_cfg.clone());

        // Manual print STILL runs even when auto_print_on_scan is false
        worker.print_with_trigger(job.clone(), PrintTrigger::Manual, disabled_cfg);

        // Missing printer produces a clear PrintFailed event
        worker.print(job, AppConfig::default());

        // RefreshPrinters returns simulated printer
        worker.refresh_printers();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while events.len() < 3 && Instant::now() < deadline {
            if let Some(ev) = worker.try_recv() {
                events.push(ev);
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }

        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            PrintEvent::PrintSucceeded {
                header: Some(h)
            } if h == "Test Label"
        ));
        assert!(matches!(&events[1], PrintEvent::PrintFailed(_)));
        assert!(matches!(
            &events[2],
            PrintEvent::PrintersLoaded(Ok(list)) if list.len() == 1
        ));
        assert!(repaints.load(Ordering::SeqCst) >= 3);
    }
}
