//! Persistent application configuration stored in `%APPDATA%\PostalAnnexQRScanner\config.json`.

use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Default GitHub repository used for architecture-aware auto-updates.
pub const DEFAULT_GITHUB_REPO: &str = "KingDomino/Postal-Annex-QR-Hub";

/// Persistent user configuration matching `SPEC.md` §6.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub printer_name: Option<String>,
    pub auto_print_on_scan: bool,
    pub force_4x6_devmode: bool,
    pub qr_size_inches: f32,
    pub label_width_inches: f32,
    pub label_height_inches: f32,
    pub offset_x_mm: f32,
    pub offset_y_mm: f32,
    pub auto_check_updates: bool,
    pub github_repo: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            printer_name: None,
            auto_print_on_scan: true,
            force_4x6_devmode: true,
            qr_size_inches: 3.125,
            label_width_inches: 4.0,
            label_height_inches: 6.0,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            auto_check_updates: true,
            github_repo: DEFAULT_GITHUB_REPO.to_string(),
        }
    }
}

/// Resolves the platform config file path:
/// - Windows: `%APPDATA%\PostalAnnexQRScanner\config.json`
/// - macOS / Linux: standard user config directory + `PostalAnnexQRScanner/config.json`
pub fn config_path() -> Option<PathBuf> {
    BaseDirs::new().map(|b| {
        b.config_dir()
            .join("PostalAnnexQRScanner")
            .join("config.json")
    })
}

impl AppConfig {
    /// Loads the config from the default [`config_path`], returning [`Self::default`]
    /// if the file does not exist or cannot be parsed.
    pub fn load() -> Self {
        match config_path() {
            Some(path) => Self::load_from(&path).unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Loads configuration from a specific JSON file path.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config {}: {e}", path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse config {}: {e}", path.display()))
    }

    /// Saves the configuration to the default [`config_path`], creating parent directories.
    pub fn save(&self) -> Result<PathBuf, String> {
        let path = config_path()
            .ok_or_else(|| "Could not determine user configuration directory".to_string())?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// Saves the configuration as pretty-printed JSON to a specific file path.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create config directory {}: {e}",
                    parent.display()
                )
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        fs::write(path, json)
            .map_err(|e| format!("Failed to write config {}: {e}", path.display()))
    }

    /// Returns the trimmed active printer name, or `None` if unconfigured or blank.
    pub fn active_printer(&self) -> Option<&str> {
        self.printer_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_defaults_missing_fields() {
        let partial = r#"{"printer_name": "ZDesigner GK420d", "offset_x_mm": 1.5}"#;
        let cfg: AppConfig = serde_json::from_str(partial).expect("should parse partial JSON");
        assert_eq!(cfg.active_printer(), Some("ZDesigner GK420d"));
        assert!(cfg.auto_print_on_scan);
        assert!(cfg.force_4x6_devmode);
        assert!((cfg.qr_size_inches - 3.125).abs() < 1e-6);
        assert!((cfg.label_width_inches - 4.0).abs() < 1e-6);
        assert!((cfg.label_height_inches - 6.0).abs() < 1e-6);
        assert!((cfg.offset_x_mm - 1.5).abs() < 1e-6);
        assert!((cfg.offset_y_mm - 0.0).abs() < 1e-6);
        assert!(cfg.auto_check_updates);
        assert_eq!(cfg.github_repo, DEFAULT_GITHUB_REPO);
    }

    #[test]
    fn save_and_load_roundtrip_on_disk() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "postal_annex_cfg_test_{}",
            std::process::id()
        ));
        let path = dir.join("nested").join("config.json");

        let cfg = AppConfig {
            printer_name: Some("Zebra ZD421".to_string()),
            auto_print_on_scan: false,
            force_4x6_devmode: true,
            qr_size_inches: 2.75,
            label_width_inches: 4.0,
            label_height_inches: 6.0,
            offset_x_mm: -1.0,
            offset_y_mm: 2.5,
            auto_check_updates: false,
            github_repo: "Example/Repo".to_string(),
        };

        cfg.save_to(&path).expect("save_to should succeed");
        let loaded = AppConfig::load_from(&path).expect("load_from should succeed");
        assert_eq!(loaded, cfg);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_printer_name_treated_as_unconfigured() {
        let cfg = AppConfig {
            printer_name: Some("   ".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.active_printer(), None);
    }
}
