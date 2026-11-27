//! Persistent application configuration stored in `%APPDATA%\QR Hub\config.json`
//! (with automatic read fallback to legacy `%APPDATA%\PostalAnnexQRScanner\config.json`).

use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Default GitHub repository used for architecture-aware auto-updates.
pub const DEFAULT_GITHUB_REPO: &str = "KingDomino/Postal-Annex-QR-Hub";

const CONFIG_DIR_NAME: &str = "QR Hub";
const LEGACY_CONFIG_DIR_NAME: &str = "PostalAnnexQRScanner";
const CONFIG_FILENAME: &str = "config.json";

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
    pub update_channel: UpdateChannel,
    pub github_repo: String,
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

/// Which GitHub releases the auto-updater follows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Full releases only (every store PC).
    #[default]
    Stable,
    /// Also pre-releases such as `v2.1.0-test.1` (for testing before promoting to stable).
    Test,
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
            update_channel: UpdateChannel::Stable,
            github_repo: DEFAULT_GITHUB_REPO.to_string(),
            last_seen_version: None,
        }
    }
}

/// Resolves the platform configuration directory:
/// - Windows: `%APPDATA%\QR Hub`
/// - macOS / Linux: standard user config directory + `QR Hub`
pub fn config_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.config_dir().join(CONFIG_DIR_NAME))
}

/// Resolves the platform config file path:
/// - Windows: `%APPDATA%\QR Hub\config.json`
/// - macOS / Linux: standard user config directory + `QR Hub/config.json`
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(CONFIG_FILENAME))
}

fn legacy_config_path() -> Option<PathBuf> {
    BaseDirs::new().map(|b| {
        b.config_dir()
            .join(LEGACY_CONFIG_DIR_NAME)
            .join(CONFIG_FILENAME)
    })
}

impl AppConfig {
    /// Loads the config from the default [`config_path`], falling back to the legacy
    /// `PostalAnnexQRScanner/config.json` location if present, or [`Self::default`]
    /// if neither exists or can be parsed.
    pub fn load() -> Self {
        Self::load_with_existing_flag().0
    }

    /// Loads configuration from [`config_path`] (or legacy `PostalAnnexQRScanner/config.json`
    /// if the new path does not exist yet), returning `(config, existed_on_disk)`.
    pub fn load_with_existing_flag() -> (Self, bool) {
        Self::load_with_fallback(config_path().as_deref(), legacy_config_path().as_deref())
    }

    /// Loads from `primary` if it exists on disk, otherwise from `legacy` if it exists.
    /// Returns `(cfg, true)` if either file was present on disk, or `(Self::default(), false)`.
    pub fn load_with_fallback(primary: Option<&Path>, legacy: Option<&Path>) -> (Self, bool) {
        if let Some(path) = primary.filter(|p| p.exists()) {
            return (Self::load_from(path).unwrap_or_default(), true);
        }
        if let Some(path) = legacy.filter(|p| p.exists()) {
            return (Self::load_from(path).unwrap_or_default(), true);
        }
        (Self::default(), false)
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
        assert_eq!(cfg.last_seen_version, None);
    }

    #[test]
    fn last_seen_version_roundtrips_and_defaults_to_none() {
        let with_ver = r#"{"last_seen_version": "2.0.2"}"#;
        let parsed: AppConfig = serde_json::from_str(with_ver).expect("should parse version");
        assert_eq!(parsed.last_seen_version.as_deref(), Some("2.0.2"));

        let serialized = serde_json::to_string(&parsed).expect("should serialize");
        let reparsed: AppConfig = serde_json::from_str(&serialized).expect("should reparse");
        assert_eq!(reparsed.last_seen_version.as_deref(), Some("2.0.2"));

        let omitted: AppConfig = serde_json::from_str("{}").expect("should parse empty object");
        assert_eq!(omitted.last_seen_version, None);
        // Configs from older versions have no channel and must stay on Stable.
        assert_eq!(omitted.update_channel, UpdateChannel::Stable);
        let test: AppConfig = serde_json::from_str(r#"{"update_channel":"test"}"#).unwrap();
        assert_eq!(test.update_channel, UpdateChannel::Test);
    }

    #[test]
    fn save_and_load_roundtrip_on_disk() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qr_hub_cfg_test_{}", std::process::id()));
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
            update_channel: UpdateChannel::Test,
            github_repo: "Example/Repo".to_string(),
            last_seen_version: Some("2.0.2".to_string()),
        };

        cfg.save_to(&path).expect("save_to should succeed");
        let loaded = AppConfig::load_from(&path).expect("load_from should succeed");
        assert_eq!(loaded, cfg);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_with_fallback_migrates_legacy_and_prefers_primary() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qr_hub_cfg_migration_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let primary = dir.join("QR Hub").join("config.json");
        let legacy = dir.join("PostalAnnexQRScanner").join("config.json");

        // Neither exists -> default, existed = false
        let (cfg, existed) = AppConfig::load_with_fallback(Some(&primary), Some(&legacy));
        assert!(!existed);
        assert_eq!(cfg, AppConfig::default());

        // Legacy exists -> loads legacy printer calibration, existed = true
        let legacy_cfg = AppConfig {
            printer_name: Some("ZDesigner GK420d".to_string()),
            offset_x_mm: 2.25,
            ..Default::default()
        };
        legacy_cfg.save_to(&legacy).expect("save legacy");
        let (cfg, existed) = AppConfig::load_with_fallback(Some(&primary), Some(&legacy));
        assert!(existed);
        assert_eq!(cfg, legacy_cfg);

        // Both exist -> primary takes precedence over legacy
        let primary_cfg = AppConfig {
            printer_name: Some("Zebra ZD421".to_string()),
            last_seen_version: Some("2.0.2".to_string()),
            ..Default::default()
        };
        primary_cfg.save_to(&primary).expect("save primary");
        let (cfg, existed) = AppConfig::load_with_fallback(Some(&primary), Some(&legacy));
        assert!(existed);
        assert_eq!(cfg, primary_cfg);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_dir_and_path_use_qr_hub_folder() {
        let dir = config_dir().expect("config_dir should resolve");
        let path = config_path().expect("config_path should resolve");
        assert_eq!(dir.file_name().unwrap(), "QR Hub");
        assert_eq!(path, dir.join("config.json"));
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
