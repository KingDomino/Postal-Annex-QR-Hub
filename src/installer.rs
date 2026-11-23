//! Zero-UAC per-user Windows self-installer, Start Menu / Desktop shortcuts, and uninstaller.
//!
//! When a release binary is launched from outside `%LOCALAPPDATA%\PostalAnnexQRScanner\`
//! (for example from Downloads or Desktop), [`handle_startup`] installs it into the
//! canonical per-user directory, creates the Start Menu shortcut, registers Add/Remove
//! Programs (`HKCU\...\Uninstall\PostalAnnexQRScanner`), launches the installed copy
//! with `--just-installed`, and asks the caller to exit. Subsequent auto-updates run
//! `self_replace` in place so all shortcuts remain valid with zero UAC prompts.

use std::path::{Path, PathBuf};

use directories::BaseDirs;

/// Full display name shown in Windows Installed Apps and shortcut tooltips.
pub const APP_DISPLAY_NAME: &str = "PostalAnnex+ Amazon QR Code Scanner";
/// Shortcut filename placed in Start Menu Programs and optionally on Desktop.
pub const SHORTCUT_FILENAME: &str = "PostalAnnex+ Amazon QR Scanner.lnk";
/// Directory name under `%LOCALAPPDATA%` holding the installed executable.
pub const INSTALL_DIR_NAME: &str = "PostalAnnexQRScanner";
/// Canonical installed executable filename.
pub const INSTALLED_EXE_NAME: &str = "PostalAnnexQRScanner.exe";
/// Per-user Uninstall registry subkey under `HKEY_CURRENT_USER`.
pub const UNINSTALL_REG_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PostalAnnexQRScanner";

/// Action returned by [`handle_startup`] telling `main()` whether to run or exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupAction {
    RunApp { just_installed: bool },
    Exit,
}

/// Parsed command-line installer and lifecycle flags.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CliFlags {
    pub install: bool,
    pub uninstall: bool,
    pub quiet: bool,
    pub portable: bool,
    pub just_installed: bool,
}

/// Parses installer CLI flags from an argument iterator (skipping argv[0] or scanning all).
pub fn parse_cli_flags<I, S>(args: I) -> CliFlags
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut flags = CliFlags::default();
    for arg in args {
        match arg.as_ref().trim().to_ascii_lowercase().as_str() {
            "--install" | "/install" => flags.install = true,
            "--uninstall" | "/uninstall" => flags.uninstall = true,
            "--quiet" | "/quiet" | "/s" => flags.quiet = true,
            "--portable" | "/portable" => flags.portable = true,
            "--just-installed" => flags.just_installed = true,
            _ => {}
        }
    }
    flags
}

/// Returns `true` if two filesystem paths refer to the same location under Windows semantics.
///
/// Strips `\\?\` / `//?/` extended path prefixes, normalizes `/` vs `\`, trims trailing
/// separators, and compares ASCII case-insensitively.
pub fn paths_equivalent(a: &Path, b: &Path) -> bool {
    normalize_path_for_cmp(a) == normalize_path_for_cmp(b)
}

fn normalize_path_for_cmp(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    let stripped = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| raw.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or(raw);
    stripped.trim_end_matches('\\').to_ascii_lowercase()
}

/// Returns the canonical per-user installation directory (`%LOCALAPPDATA%\PostalAnnexQRScanner`).
pub fn install_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.data_local_dir().join(INSTALL_DIR_NAME))
}

/// Returns the canonical installed executable path (`%LOCALAPPDATA%\PostalAnnexQRScanner\PostalAnnexQRScanner.exe`).
pub fn install_exe_path() -> Option<PathBuf> {
    install_dir().map(|dir| dir.join(INSTALLED_EXE_NAME))
}

/// Returns `true` when the current process executable is already running from [`install_exe_path`].
pub fn is_running_from_install_dir() -> bool {
    let (Ok(exe), Some(dest)) = (std::env::current_exe(), install_exe_path()) else {
        return false;
    };
    paths_equivalent(&exe, &dest)
}

/// Runs startup self-install, maintenance, or uninstall checks before opening the GUI window.
pub fn handle_startup() -> StartupAction {
    let flags = parse_cli_flags(std::env::args().skip(1));
    #[cfg(windows)]
    {
        win32_impl::handle_startup_windows(flags)
    }
    #[cfg(not(windows))]
    {
        if flags.uninstall {
            StartupAction::Exit
        } else {
            StartupAction::RunApp {
                just_installed: flags.just_installed,
            }
        }
    }
}

/// Creates or refreshes the Start Menu shortcut pointing to the installed executable.
pub fn ensure_start_menu_shortcut() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        win32_impl::ensure_start_menu_shortcut_win()
    }
    #[cfg(not(windows))]
    {
        Err("Start Menu shortcuts are only supported on Windows.".to_string())
    }
}

/// Creates or refreshes a Desktop shortcut pointing to the installed executable (or current exe).
pub fn create_desktop_shortcut() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        win32_impl::create_desktop_shortcut_win()
    }
    #[cfg(not(windows))]
    {
        Err("Desktop shortcuts are only supported on Windows.".to_string())
    }
}

#[cfg(windows)]
mod win32_impl {
    use std::ffi::{c_void, OsStr};
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HWND, RPC_E_CHANGED_MODE, S_FALSE, S_OK};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellLink,
        KF_FLAG_CREATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO,
    };

    use super::{
        install_dir, install_exe_path, paths_equivalent, CliFlags, StartupAction,
        APP_DISPLAY_NAME, INSTALLED_EXE_NAME, SHORTCUT_FILENAME, UNINSTALL_REG_SUBKEY,
    };
    use crate::config::DEFAULT_GITHUB_REPO;
    use crate::updater::CURRENT_VERSION;

    fn to_wide_null(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn path_to_wide_null(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct ComGuard {
        should_uninitialize: bool,
    }

    impl ComGuard {
        fn init() -> Result<Self, String> {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr == S_OK || hr == S_FALSE {
                Ok(Self {
                    should_uninitialize: true,
                })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    should_uninitialize: false,
                })
            } else {
                Err(format!("CoInitializeEx failed: {hr:?}"))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.should_uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct RegKeyGuard(HKEY);

    impl Drop for RegKeyGuard {
        fn drop(&mut self) {
            if !self.0 .0.is_null() {
                unsafe {
                    let _ = RegCloseKey(self.0);
                }
            }
        }
    }

    fn known_folder_path(folder_id: &GUID) -> Result<PathBuf, String> {
        let raw = unsafe { SHGetKnownFolderPath(folder_id, KF_FLAG_CREATE, HANDLE::default()) }
            .map_err(|e| format!("SHGetKnownFolderPath failed: {e}"))?;
        let path_str = unsafe { raw.to_string() }
            .map_err(|e| format!("Failed decoding Windows known folder path: {e}"))?;
        unsafe { CoTaskMemFree(Some(raw.0 as *const c_void)) };
        Ok(PathBuf::from(path_str))
    }

    fn create_shortcut(
        lnk_path: &Path,
        target_exe: &Path,
        working_dir: &Path,
        description: &str,
    ) -> Result<(), String> {
        if let Some(parent) = lnk_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed creating shortcut folder {}: {e}",
                    parent.display()
                )
            })?;
        }

        let _com = ComGuard::init()?;
        let shell_link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("CoCreateInstance(ShellLink) failed: {e}"))?;

        let target_w = path_to_wide_null(target_exe);
        let work_w = path_to_wide_null(working_dir);
        let desc_w = to_wide_null(description);
        let lnk_w = path_to_wide_null(lnk_path);

        unsafe {
            shell_link
                .SetPath(PCWSTR(target_w.as_ptr()))
                .map_err(|e| format!("IShellLinkW::SetPath failed: {e}"))?;
            shell_link
                .SetWorkingDirectory(PCWSTR(work_w.as_ptr()))
                .map_err(|e| format!("IShellLinkW::SetWorkingDirectory failed: {e}"))?;
            shell_link
                .SetDescription(PCWSTR(desc_w.as_ptr()))
                .map_err(|e| format!("IShellLinkW::SetDescription failed: {e}"))?;
            shell_link
                .SetIconLocation(PCWSTR(target_w.as_ptr()), 0)
                .map_err(|e| format!("IShellLinkW::SetIconLocation failed: {e}"))?;

            let persist: IPersistFile = shell_link
                .cast()
                .map_err(|e| format!("QueryInterface(IPersistFile) failed: {e}"))?;
            persist
                .Save(PCWSTR(lnk_w.as_ptr()), true)
                .map_err(|e| format!("IPersistFile::Save({}) failed: {e}", lnk_path.display()))?;
        }

        Ok(())
    }

    fn set_reg_sz(hkey: HKEY, name: &str, value: &str) -> Result<(), String> {
        let name_w = to_wide_null(name);
        let value_bytes: Vec<u8> = to_wide_null(value)
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        let err = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                0,
                REG_SZ,
                Some(&value_bytes),
            )
        };
        if err != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW({name}) failed: {err:?}"));
        }
        Ok(())
    }

    fn set_reg_dword(hkey: HKEY, name: &str, value: u32) -> Result<(), String> {
        let name_w = to_wide_null(name);
        let bytes = value.to_le_bytes();
        let err = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                0,
                REG_DWORD,
                Some(&bytes),
            )
        };
        if err != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW({name}) failed: {err:?}"));
        }
        Ok(())
    }

    fn register_uninstall_entry(dir: &Path, exe_path: &Path) -> Result<(), String> {
        let subkey_w = to_wide_null(UNINSTALL_REG_SUBKEY);
        let mut hkey = HKEY::default();
        let err = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW failed: {err:?}"));
        }
        let _guard = RegKeyGuard(hkey);

        let exe_str = exe_path.to_string_lossy();
        let dir_str = dir.to_string_lossy();
        let estimated_kb = fs::metadata(exe_path)
            .map(|m| ((m.len() / 1024).max(1)).min(u32::MAX as u64) as u32)
            .unwrap_or(8000);

        set_reg_sz(hkey, "DisplayName", APP_DISPLAY_NAME)?;
        set_reg_sz(hkey, "DisplayVersion", CURRENT_VERSION)?;
        set_reg_sz(hkey, "Publisher", "PostalAnnex+")?;
        set_reg_sz(hkey, "DisplayIcon", &format!("{exe_str},0"))?;
        set_reg_sz(hkey, "InstallLocation", &dir_str)?;
        set_reg_sz(hkey, "UninstallString", &format!("\"{exe_str}\" --uninstall"))?;
        set_reg_sz(
            hkey,
            "QuietUninstallString",
            &format!("\"{exe_str}\" --uninstall --quiet"),
        )?;
        set_reg_sz(
            hkey,
            "URLInfoAbout",
            &format!("https://github.com/{DEFAULT_GITHUB_REPO}"),
        )?;
        set_reg_dword(hkey, "NoModify", 1)?;
        set_reg_dword(hkey, "NoRepair", 1)?;
        set_reg_dword(hkey, "EstimatedSize", estimated_kb)?;

        Ok(())
    }

    fn install_binary(src_exe: &Path, dest_dir: &Path, dest_exe: &Path) -> Result<(), String> {
        fs::create_dir_all(dest_dir).map_err(|e| {
            format!(
                "Failed creating install directory {}: {e}",
                dest_dir.display()
            )
        })?;

        let temp_new = dest_dir.join(format!("{INSTALLED_EXE_NAME}.new"));
        let _ = fs::remove_file(&temp_new);
        fs::copy(src_exe, &temp_new).map_err(|e| {
            format!(
                "Failed copying executable to {}: {e}",
                temp_new.display()
            )
        })?;

        let mut backup_path: Option<PathBuf> = None;
        if dest_exe.exists() {
            let candidate_old = dest_dir.join(format!("{INSTALLED_EXE_NAME}.old"));
            let old_path = if fs::remove_file(&candidate_old).is_ok() || !candidate_old.exists() {
                candidate_old
            } else {
                dest_dir.join(format!(
                    "{INSTALLED_EXE_NAME}.old.{}",
                    std::process::id()
                ))
            };
            if let Err(err) = fs::rename(dest_exe, &old_path) {
                let _ = fs::remove_file(&temp_new);
                return Err(format!(
                    "Failed moving existing executable {}: {err}",
                    dest_exe.display()
                ));
            }
            backup_path = Some(old_path);
        }

        if let Err(err) = fs::rename(&temp_new, dest_exe) {
            if let Some(old) = backup_path {
                let _ = fs::rename(old, dest_exe);
            }
            let _ = fs::remove_file(&temp_new);
            return Err(format!(
                "Failed placing installed executable {}: {err}",
                dest_exe.display()
            ));
        }

        if let Some(old) = backup_path {
            let _ = fs::remove_file(old);
        }
        cleanup_old_binaries(dest_dir);

        Ok(())
    }

    fn cleanup_old_binaries(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let old_prefix = format!("{INSTALLED_EXE_NAME}.old");
        let new_name = format!("{INSTALLED_EXE_NAME}.new");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(&old_prefix) || name_str == new_name {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    fn start_menu_shortcut_path() -> Result<PathBuf, String> {
        Ok(known_folder_path(&FOLDERID_Programs)?.join(SHORTCUT_FILENAME))
    }

    fn desktop_shortcut_path() -> Result<PathBuf, String> {
        Ok(known_folder_path(&FOLDERID_Desktop)?.join(SHORTCUT_FILENAME))
    }

    pub(super) fn ensure_start_menu_shortcut_win() -> Result<PathBuf, String> {
        let dest_dir = install_dir().ok_or_else(|| "Could not locate %LOCALAPPDATA%.".to_string())?;
        let dest_exe = dest_dir.join(INSTALLED_EXE_NAME);
        let (target_exe, work_dir) = if dest_exe.exists() {
            (dest_exe, dest_dir)
        } else {
            let cur = std::env::current_exe()
                .map_err(|e| format!("Could not resolve executable path: {e}"))?;
            let work = cur.parent().map(Path::to_path_buf).unwrap_or(dest_dir);
            (cur, work)
        };
        let lnk = start_menu_shortcut_path()?;
        create_shortcut(&lnk, &target_exe, &work_dir, APP_DISPLAY_NAME)?;
        Ok(lnk)
    }

    pub(super) fn create_desktop_shortcut_win() -> Result<PathBuf, String> {
        let dest_dir = install_dir().ok_or_else(|| "Could not locate %LOCALAPPDATA%.".to_string())?;
        let dest_exe = dest_dir.join(INSTALLED_EXE_NAME);
        let (target_exe, work_dir) = if dest_exe.exists() {
            (dest_exe, dest_dir)
        } else {
            let cur = std::env::current_exe()
                .map_err(|e| format!("Could not resolve executable path: {e}"))?;
            let work = cur.parent().map(Path::to_path_buf).unwrap_or(dest_dir);
            (cur, work)
        };
        let lnk = desktop_shortcut_path()?;
        create_shortcut(&lnk, &target_exe, &work_dir, APP_DISPLAY_NAME)?;
        Ok(lnk)
    }

    fn run_uninstall(quiet: bool) {
        if !quiet {
            let prompt = to_wide_null(
                "Uninstall PostalAnnex+ Amazon QR Code Scanner?\n\nYour saved printer settings will be kept.",
            );
            let title = to_wide_null(APP_DISPLAY_NAME);
            let choice = unsafe {
                MessageBoxW(
                    HWND::default(),
                    PCWSTR(prompt.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_YESNO | MB_ICONQUESTION,
                )
            };
            if choice != IDYES {
                return;
            }
        }

        if let Ok(lnk) = start_menu_shortcut_path() {
            let _ = fs::remove_file(lnk);
        }
        if let Ok(lnk) = desktop_shortcut_path() {
            let _ = fs::remove_file(lnk);
        }

        let subkey_w = to_wide_null(UNINSTALL_REG_SUBKEY);
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(subkey_w.as_ptr()));
        }

        if let Some(dir) = install_dir() {
            cleanup_old_binaries(&dir);
            let dest_exe = dir.join(INSTALLED_EXE_NAME);
            let running_in_dir = std::env::current_exe()
                .ok()
                .is_some_and(|exe| paths_equivalent(&exe, &dest_exe));

            if running_in_dir {
                if self_replace::self_delete_outside_path(&dir).is_err() {
                    let _ = self_replace::self_delete();
                }
            } else if dest_exe.exists() {
                let _ = fs::remove_file(&dest_exe);
            }
            let _ = fs::remove_dir(&dir);
        }

        if !quiet {
            let msg = to_wide_null(
                "PostalAnnex+ Amazon QR Code Scanner has been uninstalled.",
            );
            let title = to_wide_null(APP_DISPLAY_NAME);
            unsafe {
                MessageBoxW(
                    HWND::default(),
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
        }
    }

    pub(super) fn handle_startup_windows(flags: CliFlags) -> StartupAction {
        if flags.uninstall {
            run_uninstall(flags.quiet);
            return StartupAction::Exit;
        }

        if flags.portable {
            return StartupAction::RunApp {
                just_installed: false,
            };
        }

        let (Ok(cur_exe), Some(dest_dir), Some(dest_exe)) =
            (std::env::current_exe(), install_dir(), install_exe_path())
        else {
            return StartupAction::RunApp {
                just_installed: false,
            };
        };

        if paths_equivalent(&cur_exe, &dest_exe) {
            cleanup_old_binaries(&dest_dir);
            if let Ok(lnk) = start_menu_shortcut_path() {
                if !lnk.exists() {
                    let _ = create_shortcut(&lnk, &dest_exe, &dest_dir, APP_DISPLAY_NAME);
                }
            }
            let _ = register_uninstall_entry(&dest_dir, &dest_exe);
            return StartupAction::RunApp {
                just_installed: flags.just_installed,
            };
        }

        if !cfg!(debug_assertions) || flags.install {
            if install_binary(&cur_exe, &dest_dir, &dest_exe).is_ok() {
                if let Ok(lnk) = start_menu_shortcut_path() {
                    let _ = create_shortcut(&lnk, &dest_exe, &dest_dir, APP_DISPLAY_NAME);
                }
                let _ = register_uninstall_entry(&dest_dir, &dest_exe);
                if Command::new(&dest_exe).arg("--just-installed").spawn().is_ok() {
                    return StartupAction::Exit;
                }
                return StartupAction::RunApp {
                    just_installed: true,
                };
            }
        }

        StartupAction::RunApp {
            just_installed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_flags_case_insensitively() {
        assert_eq!(parse_cli_flags::<[&str; 0], &str>([]), CliFlags::default());

        let parsed = parse_cli_flags([
            "--Install",
            "/QUIET",
            "--portable",
            "--just-installed",
            "ignored-arg",
        ]);
        assert!(parsed.install);
        assert!(!parsed.uninstall);
        assert!(parsed.quiet);
        assert!(parsed.portable);
        assert!(parsed.just_installed);

        let uninstall = parse_cli_flags(["/uninstall", "--quiet"]);
        assert!(uninstall.uninstall);
        assert!(uninstall.quiet);
    }

    #[test]
    fn paths_equivalent_handles_windows_prefixes_slashes_and_casing() {
        assert!(paths_equivalent(
            Path::new(r"\\?\C:\Users\Domino\AppData\Local\PostalAnnexQRScanner\PostalAnnexQRScanner.exe"),
            Path::new(r"c:/users/domino/appdata/local/postalannexqrscanner/postalannexqrscanner.exe")
        ));
        assert!(paths_equivalent(
            Path::new(r"C:\Apps\Scanner\"),
            Path::new(r"c:\apps\scanner")
        ));
        assert!(!paths_equivalent(
            Path::new(r"C:\Users\Domino\Downloads\PostalAnnexQRScanner.exe"),
            Path::new(r"C:\Users\Domino\AppData\Local\PostalAnnexQRScanner\PostalAnnexQRScanner.exe")
        ));
    }

    #[test]
    fn install_exe_path_uses_canonical_directory_and_filename() {
        let exe = install_exe_path().expect("install path should resolve");
        let dir = exe.parent().expect("install dir parent");
        assert_eq!(exe.file_name().unwrap(), INSTALLED_EXE_NAME);
        assert_eq!(dir.file_name().unwrap(), INSTALL_DIR_NAME);
    }
}
