//! Pure domain logic: scan capture buffer, payload parser, and application state machine.

pub mod buffer;
pub mod parser;
pub mod state;

pub use buffer::{ScanBuffer, DEFAULT_INACTIVITY_TIMEOUT};
pub use parser::{classify_scan, flip_case, normalize_caps_lock, ScanRoute};
pub use state::{
    AlertBadge, AppEffect, PrintJob, PrintTrigger, PromptKind, ScannerStateMachine, ScreenState,
};
