//! Architecture-aware GitHub Releases auto-updater with SHA-256 verification and self-replace.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{config_dir, DEFAULT_GITHUB_REPO};

/// Current application version from `Cargo.toml`.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Embedded release notes for the current version, used as an offline fallback for "What's New?".
pub const CURRENT_CHANGELOG: &str = "\
- Added a \"What's New?\" dialog that will pop up after updates\n\
- Added a button in the footer that allows you to report issues with the app\n\
- Changed window decorations to make the app more seamless\n\
- Changed the name of the app executable to QR Hub\n\
- Changed codebase to be more coherent";

/// Release asset filename for Windows x86_64 builds.
pub const ASSET_X64: &str = "QR-Hub-windows-x64.exe";

/// Release asset filename for Windows ARM64 builds.
pub const ASSET_ARM64: &str = "QR-Hub-windows-arm64.exe";

/// Combined SHA-256 checksum manifest attached to every GitHub Release.
pub const CHECKSUMS_ASSET: &str = "checksums.txt";

const CACHED_CHANGELOG_FILENAME: &str = "last-update-notes.json";

/// Release asset filename matching the currently compiled target architecture.
#[cfg(target_arch = "x86_64")]
pub const EXPECTED_ASSET: &str = ASSET_X64;

/// Release asset filename matching the currently compiled target architecture.
#[cfg(target_arch = "aarch64")]
pub const EXPECTED_ASSET: &str = ASSET_ARM64;

/// Fallback asset filename for non-x86_64/non-aarch64 targets.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const EXPECTED_ASSET: &str = ASSET_X64;

/// Human-readable label for the current CPU architecture shown in Settings.
pub fn current_arch_label() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "ARM64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        std::env::consts::ARCH
    }
}

/// Maps a target architecture identifier to its expected Windows release `.exe` filename.
pub fn expected_asset_for_arch(arch: &str) -> Result<&'static str, String> {
    match arch.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "x64" | "amd64" => Ok(ASSET_X64),
        "aarch64" | "arm64" => Ok(ASSET_ARM64),
        other => Err(format!("Unsupported architecture for auto-update: {other}")),
    }
}

/// Compares `current` version against a GitHub release `candidate_tag` (with or without leading `v`).
/// Returns `Ok(true)` iff `candidate_tag` is strictly newer than `current`.
pub fn is_newer_version(current: &str, candidate_tag: &str) -> Result<bool, String> {
    let cur = Version::parse(strip_v_prefix(current))
        .map_err(|e| format!("Invalid current version \"{current}\": {e}"))?;
    let cand = Version::parse(strip_v_prefix(candidate_tag))
        .map_err(|e| format!("Invalid release tag \"{candidate_tag}\": {e}"))?;
    Ok(cand > cur)
}

fn strip_v_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
}

/// Extracts the expected lowercase 64-character SHA-256 hex digest for `asset_name`
/// from a standard `checksums.txt` file (`<sha256>  <filename>` or `<sha256> *<filename>`).
pub fn parse_checksum_for_asset(checksums_txt: &str, asset_name: &str) -> Option<String> {
    for line in checksums_txt.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let hash = parts.next()?;
        let raw_name = parts.next()?;
        let normalized_name = raw_name
            .trim_start_matches('*')
            .trim_start_matches("./");
        if normalized_name == asset_name
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// Computes the lowercase hexadecimal SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    bytes_to_hex(&digest)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Verifies that `bytes` matches `expected_hex` (case-insensitive).
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    let actual = sha256_hex(bytes);
    let expected = expected_hex.trim().to_ascii_lowercase();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 checksum mismatch: expected {expected}, got {actual}"
        ))
    }
}

/// Builds the GitHub Issues URL for `repo`, falling back to [`DEFAULT_GITHUB_REPO`] when empty.
pub fn issues_url(repo: &str) -> String {
    let trimmed = repo.trim().trim_end_matches('/');
    let cleaned = if trimmed.is_empty() {
        DEFAULT_GITHUB_REPO
    } else {
        trimmed
    };
    format!("https://github.com/{cleaned}/issues")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedChangelog {
    version: String,
    notes: String,
}

/// Returns the path where cached update release notes are stored (`%APPDATA%\QR Hub\last-update-notes.json`).
pub fn cached_changelog_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(CACHED_CHANGELOG_FILENAME))
}

/// Persists release notes for `version` to the default cached changelog path.
pub fn save_cached_changelog(version: &str, notes: &str) -> Result<(), String> {
    let path = cached_changelog_path()
        .ok_or_else(|| "Could not determine user configuration directory".to_string())?;
    save_cached_changelog_to(&path, version, notes)
}

/// Loads cached release notes for `version` from the default cached changelog path, if they match.
pub fn load_cached_changelog(version: &str) -> Option<String> {
    let path = cached_changelog_path()?;
    load_cached_changelog_from(&path, version)
}

fn save_cached_changelog_to(path: &Path, version: &str, notes: &str) -> Result<(), String> {
    let trimmed_notes = notes.trim();
    if trimmed_notes.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create changelog cache directory: {e}"))?;
    }
    let payload = CachedChangelog {
        version: strip_v_prefix(version).to_string(),
        notes: trimmed_notes.to_string(),
    };
    let json = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("Failed to serialize cached changelog: {e}"))?;
    fs::write(path, json).map_err(|e| format!("Failed to write cached changelog: {e}"))
}

fn load_cached_changelog_from(path: &Path, version: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let cached: CachedChangelog = serde_json::from_str(&raw).ok()?;
    if strip_v_prefix(&cached.version) != strip_v_prefix(version) {
        return None;
    }
    let notes = cached.notes.trim();
    if notes.is_empty() {
        None
    } else {
        Some(notes.to_string())
    }
}

/// Metadata for a newer release ready to be downloaded and installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub tag_name: String,
    pub asset_name: String,
    pub asset_url: String,
    pub checksums_url: String,
    pub release_notes: Option<String>,
}

/// Commands processed by the background [`UpdateWorker`] thread.
#[derive(Debug, Clone)]
pub enum UpdateCommand {
    Check {
        repo: String,
        silent: bool,
        initial_delay: Option<Duration>,
    },
    Install(AvailableUpdate),
    FetchChangelog {
        repo: String,
        version: String,
    },
}

/// Events emitted by the background [`UpdateWorker`] thread.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Checking,
    UpToDate { silent: bool },
    Available(AvailableUpdate),
    DownloadProgress { downloaded: u64, total: Option<u64> },
    Verifying,
    ReadyToRestart,
    ChangelogLoaded { version: String, notes: String },
    Error { message: String, silent: bool },
}

/// Background worker thread for checking, downloading, verifying, and applying updates.
pub struct UpdateWorker {
    tx: Sender<UpdateCommand>,
    rx: Receiver<UpdateEvent>,
}

impl UpdateWorker {
    /// Spawns the background auto-updater worker thread.
    pub fn spawn(repaint_cb: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UpdateCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<UpdateEvent>();

        thread::Builder::new()
            .name("update-worker".to_string())
            .spawn(move || {
                let emit = |event: UpdateEvent| -> bool {
                    let ok = evt_tx.send(event).is_ok();
                    if ok {
                        if let Some(cb) = &repaint_cb {
                            cb();
                        }
                    }
                    ok
                };

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        UpdateCommand::Check {
                            repo,
                            silent,
                            initial_delay,
                        } => {
                            if let Some(delay) = initial_delay {
                                if !delay.is_zero() {
                                    thread::sleep(delay);
                                }
                            }
                            if !emit(UpdateEvent::Checking) {
                                break;
                            }
                            run_check(&repo, silent, &emit);
                        }
                        UpdateCommand::Install(update) => {
                            run_install(&update, &emit);
                        }
                        UpdateCommand::FetchChangelog { repo, version } => {
                            run_fetch_changelog(&repo, &version, &emit);
                        }
                    }
                }
            })
            .expect("failed to spawn update-worker thread");

        Self {
            tx: cmd_tx,
            rx: evt_rx,
        }
    }

    /// Triggers an asynchronous GitHub Releases check.
    pub fn check(&self, repo: String, silent: bool, initial_delay: Option<Duration>) {
        let _ = self.tx.send(UpdateCommand::Check {
            repo,
            silent,
            initial_delay,
        });
    }

    /// Downloads, verifies SHA-256, self-replaces the binary, and spawns the new process.
    pub fn install(&self, update: AvailableUpdate) {
        let _ = self.tx.send(UpdateCommand::Install(update));
    }

    /// Fetches live release notes for `version` from GitHub Releases (silent on offline/errors).
    pub fn fetch_changelog(&self, repo: String, version: String) {
        let _ = self
            .tx
            .send(UpdateCommand::FetchChangelog { repo, version });
    }

    /// Non-blocking poll for the next [`UpdateEvent`].
    pub fn try_recv(&self) -> Option<UpdateEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(feature = "online-updater")]
#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[cfg(feature = "online-updater")]
#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(feature = "online-updater")]
fn user_agent() -> String {
    format!("QRHub/{CURRENT_VERSION}")
}

#[cfg(feature = "online-updater")]
fn run_check(repo: &str, silent: bool, emit: &dyn Fn(UpdateEvent) -> bool) {
    let repo_trimmed = repo.trim();
    if repo_trimmed.is_empty() {
        let _ = emit(UpdateEvent::Error {
            message: "GitHub repository is not configured.".to_string(),
            silent,
        });
        return;
    }

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(user_agent())
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Failed to initialize HTTP client: {err}"),
                silent,
            });
            return;
        }
    };

    let url = format!("https://api.github.com/repos/{repo_trimmed}/releases/latest");
    let resp = match client.get(&url).send() {
        Ok(r) => r,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Could not reach GitHub Releases: {err}"),
                silent,
            });
            return;
        }
    };

    if !resp.status().is_success() {
        let _ = emit(UpdateEvent::Error {
            message: format!("GitHub Releases returned HTTP {}", resp.status()),
            silent,
        });
        return;
    }

    let release: GithubRelease = match resp.json() {
        Ok(r) => r,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Invalid GitHub release payload: {err}"),
                silent,
            });
            return;
        }
    };

    match is_newer_version(CURRENT_VERSION, &release.tag_name) {
        Ok(false) => {
            let _ = emit(UpdateEvent::UpToDate { silent });
            return;
        }
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: err,
                silent,
            });
            return;
        }
        Ok(true) => {}
    }

    let asset = release.assets.iter().find(|a| a.name == EXPECTED_ASSET);
    let checksums = release.assets.iter().find(|a| a.name == CHECKSUMS_ASSET);

    match (asset, checksums) {
        (Some(bin), Some(chk)) => {
            let _ = emit(UpdateEvent::Available(AvailableUpdate {
                version: strip_v_prefix(&release.tag_name).to_string(),
                tag_name: release.tag_name,
                asset_name: bin.name.clone(),
                asset_url: bin.browser_download_url.clone(),
                checksums_url: chk.browser_download_url.clone(),
                release_notes: release.body,
            }));
        }
        (None, _) => {
            let _ = emit(UpdateEvent::Error {
                message: format!(
                    "Release {} does not contain an installer for {} ({EXPECTED_ASSET}).",
                    release.tag_name,
                    current_arch_label()
                ),
                silent,
            });
        }
        (_, None) => {
            let _ = emit(UpdateEvent::Error {
                message: format!(
                    "Release {} is missing required {CHECKSUMS_ASSET} manifest.",
                    release.tag_name
                ),
                silent,
            });
        }
    }
}

#[cfg(feature = "online-updater")]
fn run_install(update: &AvailableUpdate, emit: &dyn Fn(UpdateEvent) -> bool) {
    use std::fs::{self, File};
    use std::io::{Read, Write};

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(user_agent())
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Failed to initialize download client: {err}"),
                silent: false,
            });
            return;
        }
    };

    // 1. Download checksums.txt and locate the expected SHA-256 for this architecture.
    let checksums_text = match client
        .get(&update.checksums_url)
        .timeout(Duration::from_secs(15))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
    {
        Ok(text) => text,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Failed to download {CHECKSUMS_ASSET}: {err}"),
                silent: false,
            });
            return;
        }
    };

    let expected_hash = match parse_checksum_for_asset(&checksums_text, &update.asset_name) {
        Some(h) => h,
        None => {
            let _ = emit(UpdateEvent::Error {
                message: format!(
                    "Checksum for {} was not found in {CHECKSUMS_ASSET}.",
                    update.asset_name
                ),
                silent: false,
            });
            return;
        }
    };

    // 2. Stream download binary into %TEMP% while computing SHA-256.
    let mut resp = match client
        .get(&update.asset_url)
        .send()
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Failed to download update binary: {err}"),
                silent: false,
            });
            return;
        }
    };

    let total = resp.content_length();
    let temp_path = std::env::temp_dir().join(format!(
        "QR-Hub-update-{}-{}.exe",
        std::process::id(),
        update.version.replace(['/', '\\', ' '], "_")
    ));

    let mut file = match File::create(&temp_path) {
        Ok(f) => f,
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Failed to create temporary update file: {err}"),
                silent: false,
            });
            return;
        }
    };

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    let mut chunk = [0u8; 32 * 1024];

    loop {
        match resp.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(err) = file.write_all(&chunk[..n]) {
                    drop(file);
                    let _ = fs::remove_file(&temp_path);
                    let _ = emit(UpdateEvent::Error {
                        message: format!("Failed writing update file: {err}"),
                        silent: false,
                    });
                    return;
                }
                hasher.update(&chunk[..n]);
                downloaded += n as u64;
                if downloaded.saturating_sub(last_reported) >= 64 * 1024
                    || total.is_some_and(|t| downloaded >= t)
                {
                    last_reported = downloaded;
                    let _ = emit(UpdateEvent::DownloadProgress { downloaded, total });
                }
            }
            Err(err) => {
                drop(file);
                let _ = fs::remove_file(&temp_path);
                let _ = emit(UpdateEvent::Error {
                    message: format!("Download interrupted: {err}"),
                    silent: false,
                });
                return;
            }
        }
    }

    if let Err(err) = file.flush() {
        drop(file);
        let _ = fs::remove_file(&temp_path);
        let _ = emit(UpdateEvent::Error {
            message: format!("Failed to flush downloaded update: {err}"),
            silent: false,
        });
        return;
    }
    drop(file);

    // 3. Verify SHA-256 digest before touching the running binary.
    let _ = emit(UpdateEvent::Verifying);
    let actual_hash = bytes_to_hex(&hasher.finalize());
    if actual_hash != expected_hash {
        let _ = fs::remove_file(&temp_path);
        let _ = emit(UpdateEvent::Error {
            message: format!(
                "Update aborted: SHA-256 verification failed (expected {expected_hash}, got {actual_hash})."
            ),
            silent: false,
        });
        return;
    }

    // 4. Cache release notes before replacing, then atomically self-replace and relaunch.
    if let Some(notes) = update
        .release_notes
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = save_cached_changelog(&update.version, notes);
    }

    if let Err(err) = self_replace::self_replace(&temp_path) {
        let _ = fs::remove_file(&temp_path);
        let _ = emit(UpdateEvent::Error {
            message: format!("Failed to replace running executable: {err}"),
            silent: false,
        });
        return;
    }
    let _ = fs::remove_file(&temp_path);

    match std::env::current_exe() {
        Ok(exe_path) => {
            if let Err(err) = std::process::Command::new(&exe_path)
                .arg("--just-updated")
                .spawn()
            {
                let _ = emit(UpdateEvent::Error {
                    message: format!("Update installed, but automatic restart failed: {err}"),
                    silent: false,
                });
                return;
            }
            let _ = emit(UpdateEvent::ReadyToRestart);
        }
        Err(err) => {
            let _ = emit(UpdateEvent::Error {
                message: format!("Update installed, but could not locate executable path: {err}"),
                silent: false,
            });
        }
    }
}

#[cfg(feature = "online-updater")]
fn run_fetch_changelog(repo: &str, version: &str, emit: &dyn Fn(UpdateEvent) -> bool) {
    let repo_trimmed = repo.trim().trim_end_matches('/');
    let ver = strip_v_prefix(version);
    if repo_trimmed.is_empty() || ver.is_empty() {
        return;
    }

    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(user_agent())
        .build()
    else {
        return;
    };

    let url = format!("https://api.github.com/repos/{repo_trimmed}/releases/tags/v{ver}");
    let Ok(resp) = client.get(&url).send() else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }

    #[derive(Deserialize)]
    struct ReleaseNotesPayload {
        body: Option<String>,
    }

    let Ok(payload) = resp.json::<ReleaseNotesPayload>() else {
        return;
    };

    if let Some(notes) = payload
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = emit(UpdateEvent::ChangelogLoaded {
            version: ver.to_string(),
            notes: notes.to_string(),
        });
    }
}

#[cfg(not(feature = "online-updater"))]
fn run_check(_repo: &str, silent: bool, emit: &dyn Fn(UpdateEvent) -> bool) {
    let _ = emit(UpdateEvent::Error {
        message: "Online auto-updater is disabled in this build.".to_string(),
        silent,
    });
}

#[cfg(not(feature = "online-updater"))]
fn run_install(_update: &AvailableUpdate, emit: &dyn Fn(UpdateEvent) -> bool) {
    let _ = emit(UpdateEvent::Error {
        message: "Online auto-updater is disabled in this build.".to_string(),
        silent: false,
    });
}

#[cfg(not(feature = "online-updater"))]
fn run_fetch_changelog(_repo: &str, _version: &str, _emit: &dyn Fn(UpdateEvent) -> bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_correct_windows_asset_per_architecture() {
        assert_eq!(ASSET_X64, "QR-Hub-windows-x64.exe");
        assert_eq!(ASSET_ARM64, "QR-Hub-windows-arm64.exe");
        assert_eq!(
            expected_asset_for_arch("x86_64").unwrap(),
            "QR-Hub-windows-x64.exe"
        );
        assert_eq!(
            expected_asset_for_arch("aarch64").unwrap(),
            "QR-Hub-windows-arm64.exe"
        );
        assert!(expected_asset_for_arch("riscv64").is_err());
    }

    #[test]
    fn issues_url_formats_and_falls_back_to_default_repo() {
        assert_eq!(
            issues_url("Owner/Repo"),
            "https://github.com/Owner/Repo/issues"
        );
        assert_eq!(
            issues_url("  Owner/Repo///  "),
            "https://github.com/Owner/Repo/issues"
        );
        assert_eq!(
            issues_url("   "),
            format!("https://github.com/{DEFAULT_GITHUB_REPO}/issues")
        );
    }

    #[test]
    fn current_changelog_is_populated() {
        assert!(CURRENT_CHANGELOG.contains("What's New?"));
        assert!(CURRENT_CHANGELOG.contains("QR Hub"));
    }

    #[test]
    fn cached_changelog_roundtrip_matches_version() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qr_hub_changelog_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("last-update-notes.json");

        save_cached_changelog_to(&path, "v2.0.2", "  - Fixed bug\n- Added feature  ")
            .expect("save should succeed");

        assert_eq!(
            load_cached_changelog_from(&path, "2.0.2").as_deref(),
            Some("- Fixed bug\n- Added feature")
        );
        assert_eq!(
            load_cached_changelog_from(&path, "v2.0.2").as_deref(),
            Some("- Fixed bug\n- Added feature")
        );
        assert_eq!(load_cached_changelog_from(&path, "2.0.3"), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn semver_comparison_handles_v_prefix() {
        assert!(is_newer_version("0.1.0", "v0.1.1").unwrap());
        assert!(is_newer_version("0.1.0", "0.2.0").unwrap());
        assert!(is_newer_version("2.0.0", "v2.0.1").unwrap());
        assert!(is_newer_version("2.0.0", "2.1.0").unwrap());
        assert!(!is_newer_version("0.1.1", "v0.1.1").unwrap());
        assert!(!is_newer_version("2.0.0", "v2.0.0").unwrap());
        assert!(!is_newer_version("0.2.0", "v0.1.9").unwrap());
        assert!(is_newer_version("0.1.0", "not-a-semver").is_err());
    }

    #[test]
    fn checksum_parsing_and_verification_accepts_match_and_rejects_mismatch() {
        let payload = b"PostalAnnex+ test binary contents";
        let valid_hash = sha256_hex(payload);
        let other_hash = "0".repeat(64);

        let manifest = format!(
            "# release checksums\n{other_hash}  {ASSET_X64}\n{valid_hash}  {ASSET_ARM64}\n"
        );

        assert_eq!(
            parse_checksum_for_asset(&manifest, ASSET_ARM64),
            Some(valid_hash.clone())
        );
        assert_eq!(
            parse_checksum_for_asset(&manifest, ASSET_X64),
            Some(other_hash.clone())
        );
        assert_eq!(parse_checksum_for_asset(&manifest, "unknown.exe"), None);

        assert!(verify_sha256(payload, &valid_hash).is_ok());
        assert!(verify_sha256(payload, &valid_hash.to_uppercase()).is_ok());
        assert!(verify_sha256(payload, &other_hash).is_err());
        assert!(verify_sha256(b"tampered bytes", &valid_hash).is_err());
    }
}
