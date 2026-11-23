//! Core library for the PostalAnnex+ Amazon QR Code Scanner.
//!
//! Exposes pure domain logic, QR encoding & animation math, persistent configuration,
//! clipboard retry helper, Win32/cross-platform thermal printing, icon rendering,
//! and the architecture-aware GitHub Releases auto-updater.

pub mod clipboard;
pub mod config;
pub mod domain;
pub mod icon;
pub mod installer;
pub mod printer;
pub mod qr;
pub mod updater;
