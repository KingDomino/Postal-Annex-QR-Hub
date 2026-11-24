//! Pure scanner application state machine and side-effect descriptions.

use super::parser::{classify_scan, ScanRoute};

/// Represents a thermal label print job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintJob {
    /// Full normalized QR payload (never just the RMA substring).
    pub payload: String,
    /// Optional header printed uppercase above the QR code (`None` for Override).
    pub header: Option<String>,
}

/// Distinguishes automatic scan-triggered printing from explicit manual print requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintTrigger {
    /// Triggered automatically by a completed scan/confirmation; respects `config.auto_print_on_scan`.
    Auto,
    /// Explicitly triggered by the associate (Override, Reprint, Test Print); always prints.
    Manual,
}

/// Visual status badge displayed above/around the generated QR label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertBadge {
    /// Green confirmation pill (`#28a745` bg, `#ffffff` text).
    Green { text: String },
    /// Yellow warning pill (`#ffc107` bg, `#212529` text).
    Yellow { text: String },
}

/// Interactive confirmation prompt awaiting a Yes/No click from the associate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    HappyReturn {
        payload: String,
        rma: Option<String>,
    },
    CustomerPacked {
        payload: String,
        rma: Option<String>,
    },
}

/// Pure UI screen state driven by scans and prompt responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenState {
    Ready,
    AwaitingPrompt(PromptKind),
    HappyReturnNote,
    PackingMaterialsNotice,
    UnrecognizedWarning {
        rejected_payload: String,
    },
    LabelReady {
        payload: String,
        header: Option<String>,
        badge: Option<AlertBadge>,
        scan_nonce: u64,
    },
}

/// Side effects emitted by [`ScannerStateMachine`] for the shell/UI layer to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    CopyToClipboard(String),
    Print {
        job: PrintJob,
        trigger: PrintTrigger,
    },
    GenerateQr {
        payload: String,
        scan_nonce: u64,
    },
}

/// Pure, deterministic state machine for the PostalAnnex+ QR Scanner workflow.
#[derive(Debug, Clone)]
pub struct ScannerStateMachine {
    screen: ScreenState,
    last_print_job: Option<PrintJob>,
    scan_nonce: u64,
}

impl Default for ScannerStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScannerStateMachine {
    /// Creates a new state machine in the [`ScreenState::Ready`] state.
    pub fn new() -> Self {
        Self {
            screen: ScreenState::Ready,
            last_print_job: None,
            scan_nonce: 0,
        }
    }

    /// Returns the current screen state.
    pub fn screen(&self) -> &ScreenState {
        &self.screen
    }

    /// Returns the last generated [`PrintJob`], if any.
    pub fn last_print_job(&self) -> Option<&PrintJob> {
        self.last_print_job.as_ref()
    }

    /// Returns a standard test print job for validating 4" x 6" thermal alignment.
    pub fn test_print_job() -> PrintJob {
        PrintJob {
            payload: "AFNLFBF|TEST-RMA-0000|POSTALANNEX-TEST".to_string(),
            header: Some("Test Label".to_string()),
        }
    }

    /// Handles a submitted scan string.
    ///
    /// Empty or whitespace-only input is ignored without altering existing state.
    /// Any non-empty input immediately flushes all previous scan state (previous QR,
    /// prompts, notices, warnings, and `last_print_job`) before routing.
    pub fn handle_scan(&mut self, raw_text: &str) -> Vec<AppEffect> {
        if raw_text.trim().is_empty() {
            return Vec::new();
        }

        // Flush all prior scan state immediately.
        self.reset();

        match classify_scan(raw_text) {
            ScanRoute::HappyReturn { payload, rma } => {
                self.screen = ScreenState::AwaitingPrompt(PromptKind::HappyReturn { payload, rma });
                Vec::new()
            }
            ScanRoute::NoBoxNoLabel { payload, rma } => {
                self.finalize_amazon_rma_label(payload, rma, "No Box No Label")
            }
            ScanRoute::CustomerPacked { payload, rma } => {
                self.screen =
                    ScreenState::AwaitingPrompt(PromptKind::CustomerPacked { payload, rma });
                Vec::new()
            }
            ScanRoute::Unpacked { payload, content } => {
                if content.is_empty() {
                    self.finalize_label(payload, None, None, None, PrintTrigger::Auto)
                } else {
                    let badge = Some(AlertBadge::Green {
                        text: format!("UNPK QR Detected! Copied: {content}"),
                    });
                    self.finalize_label(
                        payload,
                        Some(content.clone()),
                        badge,
                        Some(content),
                        PrintTrigger::Auto,
                    )
                }
            }
            ScanRoute::UpsTracking { payload } => {
                let badge = Some(AlertBadge::Green {
                    text: format!("UPS Tracking Detected! Copied: {payload}"),
                });
                self.finalize_label(
                    payload.clone(),
                    Some(payload.clone()),
                    badge,
                    Some(payload),
                    PrintTrigger::Auto,
                )
            }
            ScanRoute::Unrecognized { payload } => {
                self.screen = ScreenState::UnrecognizedWarning {
                    rejected_payload: payload,
                };
                Vec::new()
            }
        }
    }

    /// Resolves an active Yes/No confirmation prompt (`HappyReturn` or `CustomerPacked`).
    pub fn confirm_prompt(&mut self, yes: bool) -> Vec<AppEffect> {
        let prompt = match std::mem::replace(&mut self.screen, ScreenState::Ready) {
            ScreenState::AwaitingPrompt(kind) => kind,
            other => {
                self.screen = other;
                return Vec::new();
            }
        };

        match prompt {
            PromptKind::HappyReturn { payload, .. } => {
                if !yes {
                    self.screen = ScreenState::HappyReturnNote;
                    self.last_print_job = None;
                    return Vec::new();
                }

                let clipboard_text = Some(payload.clone());
                let badge = Some(AlertBadge::Yellow {
                    text: "Full QR code copied.\n⚠ Please mark the auto-detect tracking number as UPS in PostalMate, NOT Happy Returns.".to_string(),
                });
                self.finalize_label(
                    payload,
                    Some("UPS Happy Returns".to_string()),
                    badge,
                    clipboard_text,
                    PrintTrigger::Auto,
                )
            }
            PromptKind::CustomerPacked { payload, rma } => {
                if !yes {
                    self.screen = ScreenState::PackingMaterialsNotice;
                    self.last_print_job = None;
                    return Vec::new();
                }

                self.finalize_amazon_rma_label(payload, rma, "Customer Packed")
            }
        }
    }

    /// Triggers Force Print (Override) from an [`ScreenState::UnrecognizedWarning`] state.
    ///
    /// Prints the QR code with NO header and performs NO clipboard write.
    pub fn trigger_override(&mut self) -> Vec<AppEffect> {
        let payload = match std::mem::replace(&mut self.screen, ScreenState::Ready) {
            ScreenState::UnrecognizedWarning { rejected_payload } => rejected_payload,
            other => {
                self.screen = other;
                return Vec::new();
            }
        };

        self.finalize_label(payload, None, None, None, PrintTrigger::Manual)
    }

    /// Re-dispatches the last generated [`PrintJob`] without modifying the clipboard.
    pub fn trigger_reprint(&self) -> Vec<AppEffect> {
        match &self.last_print_job {
            Some(job) => vec![AppEffect::Print {
                job: job.clone(),
                trigger: PrintTrigger::Manual,
            }],
            None => Vec::new(),
        }
    }

    /// Copies the full normalized payload of the currently displayed label.
    pub fn trigger_copy_all(&self) -> Vec<AppEffect> {
        match &self.screen {
            ScreenState::LabelReady { payload, .. } => {
                vec![AppEffect::CopyToClipboard(payload.clone())]
            }
            _ => Vec::new(),
        }
    }

    /// Resets the scanner state back to [`ScreenState::Ready`] and clears the last print job.
    pub fn reset(&mut self) {
        self.screen = ScreenState::Ready;
        self.last_print_job = None;
    }

    fn finalize_amazon_rma_label(
        &mut self,
        payload: String,
        rma: Option<String>,
        header: &str,
    ) -> Vec<AppEffect> {
        let badge = rma.as_ref().map(|rma_val| AlertBadge::Green {
            text: format!("Amazon QR Detected! RMA Copied: {rma_val}"),
        });
        self.finalize_label(
            payload,
            Some(header.to_string()),
            badge,
            rma,
            PrintTrigger::Auto,
        )
    }

    fn finalize_label(
        &mut self,
        payload: String,
        header: Option<String>,
        badge: Option<AlertBadge>,
        clipboard_text: Option<String>,
        trigger: PrintTrigger,
    ) -> Vec<AppEffect> {
        let scan_nonce = self.next_scan_nonce();
        let job = PrintJob {
            payload: payload.clone(),
            header: header.clone(),
        };

        self.last_print_job = Some(job.clone());
        self.screen = ScreenState::LabelReady {
            payload: payload.clone(),
            header,
            badge,
            scan_nonce,
        };

        let mut effects = Vec::with_capacity(if clipboard_text.is_some() { 3 } else { 2 });
        if let Some(text) = clipboard_text {
            effects.push(AppEffect::CopyToClipboard(text));
        }
        effects.push(AppEffect::GenerateQr {
            payload,
            scan_nonce,
        });
        effects.push(AppEffect::Print { job, trigger });
        effects
    }

    fn next_scan_nonce(&mut self) -> u64 {
        self.scan_nonce = self.scan_nonce.wrapping_add(1);
        self.scan_nonce
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_2_no_box_no_label_with_rma() {
        let mut sm = ScannerStateMachine::new();
        let effects = sm.handle_scan("AFNLFBF|RMA12345|EXTRA");

        assert_eq!(
            effects,
            vec![
                AppEffect::CopyToClipboard("RMA12345".to_string()),
                AppEffect::GenerateQr {
                    payload: "AFNLFBF|RMA12345|EXTRA".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "AFNLFBF|RMA12345|EXTRA".to_string(),
                        header: Some("No Box No Label".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );

        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "AFNLFBF|RMA12345|EXTRA".to_string(),
                header: Some("No Box No Label".to_string()),
                badge: Some(AlertBadge::Green {
                    text: "Amazon QR Detected! RMA Copied: RMA12345".to_string(),
                }),
                scan_nonce: 1,
            }
        );
    }

    #[test]
    fn route_2_no_box_no_label_without_rma_skips_clipboard_and_badge() {
        let mut sm = ScannerStateMachine::new();
        let effects = sm.handle_scan("AFNLFBF");

        assert_eq!(
            effects,
            vec![
                AppEffect::GenerateQr {
                    payload: "AFNLFBF".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "AFNLFBF".to_string(),
                        header: Some("No Box No Label".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );

        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "AFNLFBF".to_string(),
                header: Some("No Box No Label".to_string()),
                badge: None,
                scan_nonce: 1,
            }
        );
    }

    #[test]
    fn route_3_customer_packed_yes_and_no() {
        let mut sm = ScannerStateMachine::new();
        let effects = sm.handle_scan("AFNLF|RMA999|EXTRA");
        assert!(effects.is_empty());
        assert_eq!(
            sm.screen(),
            &ScreenState::AwaitingPrompt(PromptKind::CustomerPacked {
                payload: "AFNLF|RMA999|EXTRA".to_string(),
                rma: Some("RMA999".to_string()),
            })
        );

        let yes_effects = sm.confirm_prompt(true);
        assert_eq!(
            yes_effects,
            vec![
                AppEffect::CopyToClipboard("RMA999".to_string()),
                AppEffect::GenerateQr {
                    payload: "AFNLF|RMA999|EXTRA".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "AFNLF|RMA999|EXTRA".to_string(),
                        header: Some("Customer Packed".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );
        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "AFNLF|RMA999|EXTRA".to_string(),
                header: Some("Customer Packed".to_string()),
                badge: Some(AlertBadge::Green {
                    text: "Amazon QR Detected! RMA Copied: RMA999".to_string(),
                }),
                scan_nonce: 1,
            }
        );

        // No branch
        assert!(sm.handle_scan("AFNLF|RMA999|EXTRA").is_empty());
        let no_effects = sm.confirm_prompt(false);
        assert!(no_effects.is_empty());
        assert_eq!(sm.screen(), &ScreenState::PackingMaterialsNotice);
        assert_eq!(sm.last_print_job(), None);
    }

    #[test]
    fn route_1_happy_return_yes_and_no() {
        let mut sm = ScannerStateMachine::new();
        assert!(sm.handle_scan("HR|12345|DATA").is_empty());
        assert_eq!(
            sm.screen(),
            &ScreenState::AwaitingPrompt(PromptKind::HappyReturn {
                payload: "HR|12345|DATA".to_string(),
                rma: Some("12345".to_string()),
            })
        );

        let yes_effects = sm.confirm_prompt(true);
        assert_eq!(
            yes_effects,
            vec![
                AppEffect::CopyToClipboard("HR|12345|DATA".to_string()),
                AppEffect::GenerateQr {
                    payload: "HR|12345|DATA".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "HR|12345|DATA".to_string(),
                        header: Some("UPS Happy Returns".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );
        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "HR|12345|DATA".to_string(),
                header: Some("UPS Happy Returns".to_string()),
                badge: Some(AlertBadge::Yellow {
                    text: "Full QR code copied.\n⚠ Please mark the auto-detect tracking number as UPS in PostalMate, NOT Happy Returns.".to_string(),
                }),
                scan_nonce: 1,
            }
        );

        // No branch
        assert!(sm.handle_scan("HR|12345|DATA").is_empty());
        let no_effects = sm.confirm_prompt(false);
        assert!(no_effects.is_empty());
        assert_eq!(sm.screen(), &ScreenState::HappyReturnNote);
        assert_eq!(sm.last_print_job(), None);
    }

    #[test]
    fn route_4_unrecognized_and_override_reprint_copy_all() {
        let mut sm = ScannerStateMachine::new();
        assert!(sm.handle_scan("RANDOM_QR_DATA").is_empty());
        assert_eq!(
            sm.screen(),
            &ScreenState::UnrecognizedWarning {
                rejected_payload: "RANDOM_QR_DATA".to_string(),
            }
        );

        let override_effects = sm.trigger_override();
        assert_eq!(
            override_effects,
            vec![
                AppEffect::GenerateQr {
                    payload: "RANDOM_QR_DATA".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "RANDOM_QR_DATA".to_string(),
                        header: None,
                    },
                    trigger: PrintTrigger::Manual,
                },
            ]
        );

        // Reprint re-issues headerless job without clipboard write
        assert_eq!(
            sm.trigger_reprint(),
            vec![AppEffect::Print {
                job: PrintJob {
                    payload: "RANDOM_QR_DATA".to_string(),
                    header: None,
                },
                trigger: PrintTrigger::Manual,
            }]
        );

        // Copy All copies full normalized payload
        assert_eq!(
            sm.trigger_copy_all(),
            vec![AppEffect::CopyToClipboard("RANDOM_QR_DATA".to_string())]
        );
    }

    #[test]
    fn new_scan_flushes_open_prompt_and_whitespace_scan_preserves_state() {
        let mut sm = ScannerStateMachine::new();
        sm.handle_scan("AFNLF|OLD_RMA|EXTRA");
        assert!(matches!(sm.screen(), ScreenState::AwaitingPrompt(_)));

        // Whitespace scan does NOT flush the open prompt
        assert!(sm.handle_scan("   ").is_empty());
        assert!(matches!(sm.screen(), ScreenState::AwaitingPrompt(_)));

        // New scan flushes old prompt and immediately processes NoBoxNoLabel
        let effects = sm.handle_scan("AFNLFBF|NEW_RMA|TAIL");
        assert_eq!(effects.len(), 3);
        assert_eq!(
            sm.trigger_copy_all(),
            vec![AppEffect::CopyToClipboard("AFNLFBF|NEW_RMA|TAIL".to_string())]
        );

        // Whitespace scan does NOT flush LabelReady or last_print_job
        assert!(sm.handle_scan("").is_empty());
        assert!(sm.last_print_job().is_some());
    }

    #[test]
    fn reset_returns_to_ready_and_clears_last_print_job() {
        let mut sm = ScannerStateMachine::new();

        // From LabelReady
        sm.handle_scan("AFNLFBF|RMA777|TAIL");
        assert!(matches!(sm.screen(), ScreenState::LabelReady { .. }));
        assert!(sm.last_print_job().is_some());
        sm.reset();
        assert_eq!(sm.screen(), &ScreenState::Ready);
        assert!(sm.last_print_job().is_none());
        assert!(sm.trigger_reprint().is_empty());
        assert!(sm.trigger_copy_all().is_empty());

        // From AwaitingPrompt
        sm.handle_scan("AFNLF|RMA888|TAIL");
        assert!(matches!(sm.screen(), ScreenState::AwaitingPrompt(_)));
        sm.reset();
        assert_eq!(sm.screen(), &ScreenState::Ready);
        assert!(sm.last_print_job().is_none());

        // From UnrecognizedWarning
        sm.handle_scan("RANDOM_DATA");
        assert!(matches!(sm.screen(), ScreenState::UnrecognizedWarning { .. }));
        sm.reset();
        assert_eq!(sm.screen(), &ScreenState::Ready);
        assert!(sm.last_print_job().is_none());
    }

    #[test]
    fn route_unpacked_prints_and_copies_suffix_while_encoding_full_qr() {
        let mut sm = ScannerStateMachine::new();
        let effects = sm.handle_scan("unpk-Box789-Item");

        assert_eq!(
            effects,
            vec![
                AppEffect::CopyToClipboard("bOX789-iTEM".to_string()),
                AppEffect::GenerateQr {
                    payload: "UNPK-bOX789-iTEM".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "UNPK-bOX789-iTEM".to_string(),
                        header: Some("bOX789-iTEM".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );

        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "UNPK-bOX789-iTEM".to_string(),
                header: Some("bOX789-iTEM".to_string()),
                badge: Some(AlertBadge::Green {
                    text: "UNPK QR Detected! Copied: bOX789-iTEM".to_string(),
                }),
                scan_nonce: 1,
            }
        );

        // Literal "UNPK-" with empty suffix skips header/clipboard/badge while still generating QR
        let empty_effects = sm.handle_scan("UNPK-");
        assert_eq!(
            empty_effects,
            vec![
                AppEffect::GenerateQr {
                    payload: "UNPK-".to_string(),
                    scan_nonce: 2,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "UNPK-".to_string(),
                        header: None,
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );
    }

    #[test]
    fn route_ups_1z_prints_and_copies_full_payload_immediately() {
        let mut sm = ScannerStateMachine::new();
        let effects = sm.handle_scan("1z12e33y3451233145");

        assert_eq!(
            effects,
            vec![
                AppEffect::CopyToClipboard("1Z12E33Y3451233145".to_string()),
                AppEffect::GenerateQr {
                    payload: "1Z12E33Y3451233145".to_string(),
                    scan_nonce: 1,
                },
                AppEffect::Print {
                    job: PrintJob {
                        payload: "1Z12E33Y3451233145".to_string(),
                        header: Some("1Z12E33Y3451233145".to_string()),
                    },
                    trigger: PrintTrigger::Auto,
                },
            ]
        );

        assert_eq!(
            sm.screen(),
            &ScreenState::LabelReady {
                payload: "1Z12E33Y3451233145".to_string(),
                header: Some("1Z12E33Y3451233145".to_string()),
                badge: Some(AlertBadge::Green {
                    text: "UPS Tracking Detected! Copied: 1Z12E33Y3451233145".to_string(),
                }),
                scan_nonce: 1,
            }
        );
    }
}
