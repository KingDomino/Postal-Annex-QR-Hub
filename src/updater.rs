//! Architecture-aware GitHub Releases auto-updater with SHA-256 verification and self-replace.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::config::{config_dir, UpdateChannel, DEFAULT_GITHUB_REPO};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Current application version from `Cargo.toml`.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Embedded release notes for the current version, used as an offline fallback for "What's New?".
pub const CURRENT_CHANGELOG: &str = "\
- Statically linked the Windows C runtime to fix startup errors on clean Windows and ARM64 machines\n\
- Updated \"What's New?\" to show all unread release changelogs with separate headers per version when skipping updates\n\
- Updated Happy Returns printed label header to \"UPS Happy Returns\"\n\
- Added support for \"UNPK-\" QR codes (prints and copies content after \"-\", while encoding the full QR code)\n\
- Added support for \"1Z\" UPS tracking QR codes (prints and copies full code immediately with Caps Lock correction)";

const CHANGELOG_2_0_2: &str = "\
- Added a \"What's New?\" dialog that will pop up after updates\n\
- Added a button in the footer that allows you to report issues with the app\n\
- Changed window decorations to make the app more seamless\n\
- Changed the name of the app executable to QR Hub\n\
- Changed codebase to be more coherent";

/// Embedded release notes by version (newest first), used offline and when a release tag is not yet published.
pub const EMBEDDED_CHANGELOGS: &[(&str, &str)] =
    &[("2.0.3", CURRENT_CHANGELOG), ("2.0.2", CHANGELOG_2_0_2)];

/// A single version's changelog section shown inside the "What's New?" dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionChangelog {
    pub version: String,
    pub notes: String,
}

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

/// Compares `current` version against a GitHub release `candidate_tag` (with or without leading `v`).
/// Returns `Ok(true)` iff `candidate_tag` is strictly newer than `current`.
pub fn is_newer_version(current: &str, candidate_tag: &str) -> Result<bool, String> {
    let cur = Version::parse(strip_v_prefix(current))
        .map_err(|e| format!("Invalid current version \"{current}\": {e}"))?;
    let cand = Version::parse(strip_v_prefix(candidate_tag))
        .map_err(|e| format!("Invalid release tag \"{candidate_tag}\": {e}"))?;
    Ok(cand > cur)
}

/// Returns the tag with the highest semantic version (unparseable tags are ignored).
/// `2.1.0-test.1` sorts below `2.1.0`, so a promoted release always wins over its tests.
pub fn newest_tag<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    tags.into_iter()
        .filter_map(|tag| Version::parse(strip_v_prefix(tag)).ok().map(|v| (v, tag)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, tag)| tag)
}

/// Trims whitespace and one leading `v`/`V` from a version tag (`" v2.0.3"` -> `"2.0.3"`).
pub fn strip_v_prefix(raw: &str) -> &str {
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
        let (Some(hash), Some(raw_name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let normalized_name = raw_name.trim_start_matches('*').trim_start_matches("./");
        if normalized_name == asset_name
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// Builds the GitHub Issues URL for `repo`, falling back to [`DEFAULT_GITHUB_REPO`] when empty.
pub fn issues_url(repo: &str) -> String {
    let repo = normalize_repo(repo).unwrap_or(DEFAULT_GITHUB_REPO);
    format!("https://github.com/{repo}/issues")
}

/// Normalizes a user-entered repository to `owner/repo`, accepting a pasted
/// `https://github.com/owner/repo` URL. Returns `None` for anything else, so stray
/// scanner input in the Settings field can never be spliced into an API URL.
pub fn normalize_repo(repo: &str) -> Option<&str> {
    let repo = repo.trim();
    let repo = ["https://github.com/", "http://github.com/", "github.com/"]
        .iter()
        .find_map(|prefix| repo.strip_prefix(prefix))
        .unwrap_or(repo)
        .trim_matches('/');
    let valid_segment = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    let (owner, name) = repo.split_once('/')?;
    (valid_segment(owner) && valid_segment(name)).then_some(repo)
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

/// Filters and orders changelogs for versions in `(last_seen, current]`, newest first.
///
/// Earlier entries in `available` take precedence when multiple entries exist for the same version.
/// When `last_seen` is `None` (e.g. upgrading from v2.0.1 or earlier), `2.0.1` is used as the
/// baseline so skipped `2.0.2`+ notes are all surfaced. If no unread entries match, falls back to
/// the single entry for `current`.
pub fn select_unread_changelogs(
    last_seen: Option<&str>,
    current: &str,
    available: Vec<VersionChangelog>,
) -> Vec<VersionChangelog> {
    let current_clean = strip_v_prefix(current);
    let Ok(current_ver) = Version::parse(current_clean) else {
        return vec![VersionChangelog {
            version: current_clean.to_string(),
            notes: CURRENT_CHANGELOG.to_string(),
        }];
    };

    let since_ver = last_seen
        .and_then(|s| Version::parse(strip_v_prefix(s)).ok())
        .unwrap_or_else(|| Version::new(2, 0, 1));

    let mut deduped: Vec<(Version, VersionChangelog)> = Vec::new();
    for entry in available {
        let trimmed_notes = entry.notes.trim();
        if trimmed_notes.is_empty() {
            continue;
        }
        let Ok(ver) = Version::parse(strip_v_prefix(&entry.version)) else {
            continue;
        };
        if !deduped.iter().any(|(existing_ver, _)| existing_ver == &ver) {
            let ver_str = ver.to_string();
            deduped.push((
                ver,
                VersionChangelog {
                    version: ver_str,
                    notes: trimmed_notes.to_string(),
                },
            ));
        }
    }

    let mut unread: Vec<(Version, VersionChangelog)> = deduped
        .iter()
        .filter(|(ver, _)| ver > &since_ver && ver <= &current_ver)
        .cloned()
        .collect();
    unread.sort_by(|a, b| b.0.cmp(&a.0));

    if !unread.is_empty() {
        return unread.into_iter().map(|(_, entry)| entry).collect();
    }

    if let Some((_, current_entry)) = deduped.into_iter().find(|(ver, _)| ver == &current_ver) {
        return vec![current_entry];
    }

    vec![VersionChangelog {
        version: current_ver.to_string(),
        notes: CURRENT_CHANGELOG.to_string(),
    }]
}

/// Builds the initial offline/cached list of unread changelogs from `last_seen` up to `current`.
pub fn initial_unread_changelogs(last_seen: Option<&str>, current: &str) -> Vec<VersionChangelog> {
    select_unread_changelogs(last_seen, current, local_changelogs(current))
}

/// Offline changelog sources: cached notes for `current` (if any), then the embedded notes.
fn local_changelogs(current: &str) -> Vec<VersionChangelog> {
    let current = strip_v_prefix(current);
    let cached = load_cached_changelog(current).map(|notes| VersionChangelog {
        version: current.to_string(),
        notes,
    });
    let embedded = EMBEDDED_CHANGELOGS
        .iter()
        .map(|&(version, notes)| VersionChangelog {
            version: version.to_string(),
            notes: notes.to_string(),
        });
    cached.into_iter().chain(embedded).collect()
}

/// Metadata for a newer release ready to be downloaded and installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
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
        channel: UpdateChannel,
        silent: bool,
        initial_delay: Option<Duration>,
    },
    Install(AvailableUpdate),
    FetchChangelog {
        repo: String,
        since_version: Option<String>,
        current_version: String,
    },
}

/// Events emitted by the background [`UpdateWorker`] thread.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Checking,
    UpToDate {
        silent: bool,
    },
    Available(AvailableUpdate),
    DownloadProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    Verifying,
    ReadyToRestart,
    ChangelogsLoaded(Vec<VersionChangelog>),
    /// An update *check* failed (`silent` for background checks).
    Error {
        message: String,
        silent: bool,
    },
    /// Downloading, verifying, or applying an update failed.
    InstallFailed(String),
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
                            channel,
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
                            run_check(&repo, channel, silent, &emit);
                        }
                        UpdateCommand::Install(update) => {
                            run_install(&update, &emit);
                        }
                        UpdateCommand::FetchChangelog {
                            repo,
                            since_version,
                            current_version,
                        } => {
                            run_fetch_changelog(
                                &repo,
                                since_version.as_deref(),
                                &current_version,
                                &emit,
                            );
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
    pub fn check(
        &self,
        repo: String,
        channel: UpdateChannel,
        silent: bool,
        initial_delay: Option<Duration>,
    ) {
        let _ = self.tx.send(UpdateCommand::Check {
            repo,
            channel,
            silent,
            initial_delay,
        });
    }

    /// Downloads, verifies SHA-256, self-replaces the binary, and spawns the new process.
    pub fn install(&self, update: AvailableUpdate) {
        let _ = self.tx.send(UpdateCommand::Install(update));
    }

    /// Fetches live release notes for all unread releases between `since_version` and `current_version`.
    pub fn fetch_changelog(
        &self,
        repo: String,
        since_version: Option<String>,
        current_version: String,
    ) {
        let _ = self.tx.send(UpdateCommand::FetchChangelog {
            repo,
            since_version,
            current_version,
        });
    }

    /// Non-blocking poll for the next [`UpdateEvent`].
    pub fn try_recv(&self) -> Option<UpdateEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(feature = "online-updater")]
mod online {
    use std::fs::{self, File};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::*;

    /// Emit a progress event at most once per this many downloaded bytes.
    const PROGRESS_STEP_BYTES: u64 = 64 * 1024;

    #[derive(Deserialize)]
    struct GithubRelease {
        tag_name: String,
        body: Option<String>,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        assets: Vec<GithubAsset>,
    }

    #[derive(Deserialize)]
    struct GithubAsset {
        name: String,
        browser_download_url: String,
    }

    /// Lowercase hex encoding, matching the digests in `checksums.txt`.
    pub(super) fn bytes_to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Deletes the temporary download on every exit path (success included).
    struct TempFile(PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn http_client(timeout: Duration) -> reqwest::Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .timeout(timeout)
            .https_only(true)
            .user_agent(format!("QRHub/{CURRENT_VERSION}"))
            .build()
    }

    pub(super) fn run_check(
        repo: &str,
        channel: UpdateChannel,
        silent: bool,
        emit: &dyn Fn(UpdateEvent) -> bool,
    ) {
        let event = match check_latest_release(repo, channel) {
            Ok(Some(update)) => UpdateEvent::Available(update),
            Ok(None) => UpdateEvent::UpToDate { silent },
            Err(message) => UpdateEvent::Error { message, silent },
        };
        let _ = emit(event);
    }

    /// Fetches and decodes a GitHub API JSON response.
    fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
        let resp = http_client(Duration::from_secs(5))
            .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?
            .get(url)
            .send()
            .map_err(|e| format!("Could not reach GitHub Releases: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GitHub Releases returned HTTP {}", resp.status()));
        }
        resp.json()
            .map_err(|e| format!("Invalid GitHub release payload: {e}"))
    }

    /// Returns `Some(update)` when the newest release on `channel` is newer than this build.
    ///
    /// Stable uses GitHub's "latest release", which never includes pre-releases.
    /// Test picks the highest version among recent published releases, pre-releases included.
    fn check_latest_release(
        repo: &str,
        channel: UpdateChannel,
    ) -> Result<Option<AvailableUpdate>, String> {
        let repo = normalize_repo(repo).ok_or_else(|| {
            "GitHub repository must look like owner/repo (check Settings).".to_string()
        })?;
        let api = format!("https://api.github.com/repos/{repo}/releases");

        let release: GithubRelease = match channel {
            UpdateChannel::Stable => get_json(&format!("{api}/latest"))?,
            UpdateChannel::Test => {
                let releases: Vec<GithubRelease> = get_json(&format!("{api}?per_page=20"))?;
                let tags = releases
                    .iter()
                    .filter(|r| !r.draft)
                    .map(|r| r.tag_name.as_str());
                let newest = newest_tag(tags)
                    .ok_or_else(|| "No published releases found.".to_string())?
                    .to_string();
                releases
                    .into_iter()
                    .find(|r| r.tag_name == newest)
                    .expect("newest tag comes from this list")
            }
        };

        if !is_newer_version(CURRENT_VERSION, &release.tag_name)? {
            return Ok(None);
        }

        let find_asset = |name: &str| release.assets.iter().find(|a| a.name == name);
        let binary = find_asset(EXPECTED_ASSET).ok_or_else(|| {
            format!(
                "Release {} does not contain an installer for {} ({EXPECTED_ASSET}).",
                release.tag_name,
                current_arch_label()
            )
        })?;
        let checksums = find_asset(CHECKSUMS_ASSET).ok_or_else(|| {
            format!(
                "Release {} is missing required {CHECKSUMS_ASSET} manifest.",
                release.tag_name
            )
        })?;

        Ok(Some(AvailableUpdate {
            version: strip_v_prefix(&release.tag_name).to_string(),
            asset_name: binary.name.clone(),
            asset_url: binary.browser_download_url.clone(),
            checksums_url: checksums.browser_download_url.clone(),
            release_notes: release.body,
        }))
    }

    pub(super) fn run_install(update: &AvailableUpdate, emit: &dyn Fn(UpdateEvent) -> bool) {
        if let Err(message) = install_update(update, emit) {
            let _ = emit(UpdateEvent::InstallFailed(message));
        }
    }

    /// Downloads, verifies SHA-256, self-replaces the binary, and relaunches it.
    fn install_update(
        update: &AvailableUpdate,
        emit: &dyn Fn(UpdateEvent) -> bool,
    ) -> Result<(), String> {
        let client = http_client(Duration::from_secs(120))
            .map_err(|e| format!("Failed to initialize download client: {e}"))?;

        // 1. Look up the expected SHA-256 for this architecture.
        let checksums_text = client
            .get(&update.checksums_url)
            .timeout(Duration::from_secs(15))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|e| format!("Failed to download {CHECKSUMS_ASSET}: {e}"))?;
        let expected_hash = parse_checksum_for_asset(&checksums_text, &update.asset_name)
            .ok_or_else(|| {
                format!(
                    "Checksum for {} was not found in {CHECKSUMS_ASSET}.",
                    update.asset_name
                )
            })?;

        // 2. Stream the binary into %TEMP% while hashing it.
        let temp = TempFile(std::env::temp_dir().join(format!(
            "QR-Hub-update-{}-{}.exe",
            std::process::id(),
            update.version.replace(['/', '\\', ' '], "_")
        )));
        let actual_hash = download_hashed(&client, &update.asset_url, &temp.0, emit)?;

        // 3. Verify before touching the running binary.
        let _ = emit(UpdateEvent::Verifying);
        if actual_hash != expected_hash {
            return Err(format!(
                "Update aborted: SHA-256 verification failed (expected {expected_hash}, got {actual_hash})."
            ));
        }

        // 4. Cache release notes, swap the executable, and relaunch.
        if let Some(notes) = update.release_notes.as_deref() {
            let _ = save_cached_changelog(&update.version, notes);
        }
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Could not locate the running executable: {e}"))?;
        if let Err(e) = self_replace::self_replace(&temp.0) {
            // self_replace moves the old exe aside before copying the new one in. If that
            // copy failed, nothing is left at `exe_path` and the old binary is deleted on
            // exit, so restore from the verified download while we still have it.
            if !exe_path.exists() && fs::copy(&temp.0, &exe_path).is_err() {
                return Err(format!(
                    "Update failed and QR Hub could not be restored ({e}). Please reinstall it from GitHub Releases."
                ));
            }
            return Err(format!("Failed to replace running executable: {e}"));
        }
        drop(temp);

        let mut relaunch = std::process::Command::new(&exe_path);
        relaunch.arg("--just-updated");
        if std::env::args()
            .any(|a| matches!(a.to_ascii_lowercase().as_str(), "--portable" | "/portable"))
        {
            relaunch.arg("--portable");
        }
        relaunch.spawn().map_err(|e| {
            format!(
                "Update installed. Please close and reopen QR Hub (automatic restart failed: {e})."
            )
        })?;
        let _ = emit(UpdateEvent::ReadyToRestart);
        Ok(())
    }

    /// Streams `url` into `dest`, reporting progress, and returns the lowercase SHA-256 hex.
    fn download_hashed(
        client: &reqwest::blocking::Client,
        url: &str,
        dest: &Path,
        emit: &dyn Fn(UpdateEvent) -> bool,
    ) -> Result<String, String> {
        let mut resp = client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("Failed to download update binary: {e}"))?;
        let total = resp.content_length();
        let mut file = File::create(dest)
            .map_err(|e| format!("Failed to create temporary update file: {e}"))?;

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut last_reported: u64 = 0;
        let mut chunk = [0u8; 32 * 1024];
        loop {
            let n = match resp.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("Download interrupted: {e}")),
            };
            if n == 0 {
                break;
            }
            file.write_all(&chunk[..n])
                .map_err(|e| format!("Failed writing update file: {e}"))?;
            hasher.update(&chunk[..n]);
            downloaded += n as u64;
            if downloaded - last_reported >= PROGRESS_STEP_BYTES
                || total.is_some_and(|t| downloaded >= t)
            {
                last_reported = downloaded;
                let _ = emit(UpdateEvent::DownloadProgress { downloaded, total });
            }
        }
        file.flush()
            .map_err(|e| format!("Failed to flush downloaded update: {e}"))?;
        Ok(bytes_to_hex(&hasher.finalize()))
    }

    pub(super) fn run_fetch_changelog(
        repo: &str,
        since_version: Option<&str>,
        current_version: &str,
        emit: &dyn Fn(UpdateEvent) -> bool,
    ) {
        let current = strip_v_prefix(current_version);
        if current.is_empty() {
            return;
        }
        let Some(mut candidates) = fetch_release_notes(repo, current).filter(|c| !c.is_empty())
        else {
            return;
        };
        // Live GitHub notes come first so they win; local notes cover unpublished versions.
        candidates.extend(local_changelogs(current));
        let selected = select_unread_changelogs(since_version, current, candidates);
        if !selected.is_empty() {
            let _ = emit(UpdateEvent::ChangelogsLoaded(selected));
        }
    }

    /// Fetches non-empty release notes for the 20 most recent non-draft releases.
    ///
    /// Pre-release notes are skipped unless this build *is* that pre-release, so stable
    /// PCs never list test builds in "What's New?".
    fn fetch_release_notes(repo: &str, current: &str) -> Option<Vec<VersionChangelog>> {
        let repo = normalize_repo(repo)?;
        let releases: Vec<GithubRelease> = get_json(&format!(
            "https://api.github.com/repos/{repo}/releases?per_page=20"
        ))
        .ok()?;
        Some(
            releases
                .into_iter()
                .filter(|r| !r.draft)
                .filter(|r| !r.prerelease || strip_v_prefix(&r.tag_name) == current)
                .filter_map(|r| {
                    let notes = r.body.as_deref()?.trim();
                    (!notes.is_empty()).then(|| VersionChangelog {
                        version: strip_v_prefix(&r.tag_name).to_string(),
                        notes: notes.to_string(),
                    })
                })
                .collect(),
        )
    }
}

#[cfg(feature = "online-updater")]
use online::{run_check, run_fetch_changelog, run_install};

#[cfg(not(feature = "online-updater"))]
fn run_check(
    _repo: &str,
    _channel: UpdateChannel,
    silent: bool,
    emit: &dyn Fn(UpdateEvent) -> bool,
) {
    let _ = emit(UpdateEvent::Error {
        message: "Online auto-updater is disabled in this build.".to_string(),
        silent,
    });
}

#[cfg(not(feature = "online-updater"))]
fn run_install(_update: &AvailableUpdate, emit: &dyn Fn(UpdateEvent) -> bool) {
    let _ = emit(UpdateEvent::InstallFailed(
        "Online auto-updater is disabled in this build.".to_string(),
    ));
}

#[cfg(not(feature = "online-updater"))]
fn run_fetch_changelog(
    _repo: &str,
    _since_version: Option<&str>,
    _current_version: &str,
    _emit: &dyn Fn(UpdateEvent) -> bool,
) {
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn selects_correct_windows_asset_per_architecture() {
        assert_eq!(ASSET_X64, "QR-Hub-windows-x64.exe");
        assert_eq!(ASSET_ARM64, "QR-Hub-windows-arm64.exe");
        #[cfg(target_arch = "aarch64")]
        assert_eq!(EXPECTED_ASSET, ASSET_ARM64);
        #[cfg(not(target_arch = "aarch64"))]
        assert_eq!(EXPECTED_ASSET, ASSET_X64);
    }

    #[test]
    fn newest_embedded_changelog_matches_cargo_version() {
        // Bumping Cargo.toml without adding notes would show stale notes under a new header.
        // Test builds (`2.1.0-test.1`) share the notes of the release they lead up to.
        let mut current = Version::parse(CURRENT_VERSION).unwrap();
        current.pre = semver::Prerelease::EMPTY;
        assert_eq!(EMBEDDED_CHANGELOGS[0].0, current.to_string());
    }

    #[test]
    fn newest_tag_orders_prereleases_below_their_release() {
        let tags = ["v2.0.3", "v2.1.0-test.1", "v2.1.0-test.2", "not-a-version"];
        assert_eq!(newest_tag(tags), Some("v2.1.0-test.2"));
        assert_eq!(
            newest_tag(["v2.1.0-test.2", "v2.1.0", "v2.0.3"]),
            Some("v2.1.0")
        );
        assert_eq!(
            newest_tag(["v2.1.0-test.10", "v2.1.0-test.9"]),
            Some("v2.1.0-test.10")
        );
        assert_eq!(newest_tag([]), None);

        // A test PC moves onto the promoted release; a stable PC never sees test builds as newer.
        assert!(is_newer_version("2.1.0-test.2", "v2.1.0").unwrap());
        assert!(!is_newer_version("2.1.0-test.2", "v2.0.3").unwrap());
    }

    #[test]
    fn normalize_repo_accepts_owner_repo_and_urls_only() {
        assert_eq!(normalize_repo(" Owner/Repo/ "), Some("Owner/Repo"));
        assert_eq!(
            normalize_repo("https://github.com/KingDomino/Postal-Annex-QR-Hub"),
            Some("KingDomino/Postal-Annex-QR-Hub")
        );
        assert_eq!(normalize_repo("my.org/my_repo-2"), Some("my.org/my_repo-2"));
        for bad in [
            "",
            "owner",
            "/repo",
            "a/b/c",
            "a/b?x=1",
            "a/b#frag",
            "a b/c",
            "owner/repoAFNLFBF|RMA1",
        ] {
            assert_eq!(normalize_repo(bad), None, "{bad:?} should be rejected");
        }
        assert_eq!(
            issues_url("garbage"),
            format!("https://github.com/{DEFAULT_GITHUB_REPO}/issues")
        );
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
        assert!(CURRENT_CHANGELOG.contains("UPS Happy Returns"));
        assert!(CURRENT_CHANGELOG.contains("UNPK-"));
        assert!(CURRENT_CHANGELOG.contains("1Z"));
    }

    #[test]
    fn select_unread_changelogs_combines_skipped_versions_newest_first() {
        let releases = vec![
            VersionChangelog {
                version: "v2.0.3".to_string(),
                notes: "- 2.0.3 live notes".to_string(),
            },
            VersionChangelog {
                version: "2.0.3".to_string(),
                notes: "- 2.0.3 duplicate should lose".to_string(),
            },
            VersionChangelog {
                version: "2.0.2".to_string(),
                notes: "- 2.0.2 live notes".to_string(),
            },
            VersionChangelog {
                version: "2.0.1".to_string(),
                notes: "- 2.0.1 older notes".to_string(),
            },
        ];

        // Skipping v2.0.2 (2.0.1 -> 2.0.3) returns both 2.0.3 and 2.0.2 newest-first
        let skipped = select_unread_changelogs(Some("2.0.1"), "2.0.3", releases.clone());
        assert_eq!(
            skipped,
            vec![
                VersionChangelog {
                    version: "2.0.3".to_string(),
                    notes: "- 2.0.3 live notes".to_string(),
                },
                VersionChangelog {
                    version: "2.0.2".to_string(),
                    notes: "- 2.0.2 live notes".to_string(),
                },
            ]
        );

        // Missing last_seen_version (upgrading from pre-2.0.2 install) also returns 2.0.3 + 2.0.2
        let legacy = select_unread_changelogs(None, "2.0.3", releases.clone());
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[0].version, "2.0.3");
        assert_eq!(legacy[1].version, "2.0.2");

        // Stepping 2.0.2 -> 2.0.3 shows only 2.0.3
        let single = select_unread_changelogs(Some("2.0.2"), "2.0.3", releases.clone());
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].version, "2.0.3");

        // Reopening manually on same version falls back to current version's notes
        let manual = select_unread_changelogs(Some("2.0.3"), "2.0.3", releases);
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].version, "2.0.3");
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
        let valid_hash: String = Sha256::digest(payload)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
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

        // A malformed line must not abort the search for later valid entries.
        let messy = format!("garbage\n{valid_hash} *./{ASSET_X64}\n");
        assert_eq!(
            parse_checksum_for_asset(&messy, ASSET_X64),
            Some(valid_hash.clone())
        );

        // Uppercase digests in the manifest are normalized to match our lowercase hex.
        let upper = format!("{}  {ASSET_X64}\n", valid_hash.to_uppercase());
        assert_eq!(
            parse_checksum_for_asset(&upper, ASSET_X64),
            Some(valid_hash.clone())
        );
    }
}
