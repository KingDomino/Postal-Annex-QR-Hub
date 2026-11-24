# QR Hub — PostalAnnex+ Amazon QR Code Scanner

Native, offline-first Windows desktop application written in Rust (`eframe` / `egui`) for fast Amazon Return & Happy Return QR scanning, clipboard automation, and direct 4" x 6" thermal label printing via Win32 GDI.

## Features

- **Zero-Friction Scanning:** Opens straight to **"Ready to Scan"** with an invisible root-level scan capture buffer that automatically clears stray keystrokes after 1 second of inactivity.
- **100% Counter Business Rules:** Caps Lock case-inversion engine, 4-route classification (`Happy Return`, `No Box No Label`, `Customer Packed`, and fallback `Override`), RMA/payload clipboard copy with retry-on-lock, `Copy All`, and `Reprint Last Label`.
- **Direct 4" x 6" Win32 Thermal Printing:** Prints immediately via the Windows spooler (`DEVMODEW` 4" x 6" portrait, integer-snapped QR modules at native printer DPI) with zero print dialogs.
- **Zero-UAC Start Menu Installer & Auto-Updater:** Download and run the single `.exe` once — it installs itself per-user into `%LOCALAPPDATA%\QR Hub\`, adds a **QR Hub** Start Menu shortcut, registers under Windows **Installed Apps**, supports one-click SHA-256 verified GitHub Release updates, and shows a **"What's New?"** dialog after updating.
- **Built-In Issue Reporting:** Click **"Report an Issue"** in the app footer at any time to open the GitHub Issues tracker directly.

## Installation (Windows x64 & ARM64)

1. Download the matching executable from [GitHub Releases](https://github.com/KingDomino/Postal-Annex-QR-Hub/releases):
   - **Standard Windows PCs (Intel / AMD 64-bit):** `QR-Hub-windows-x64.exe`
   - **Windows on ARM64 (Snapdragon / Surface ARM):** `QR-Hub-windows-arm64.exe`
2. Double-click the downloaded `.exe` (no administrator/UAC prompt required).
   - It automatically installs into `%LOCALAPPDATA%\QR Hub\QR Hub.exe`, creates a **QR Hub** shortcut in the Start Menu, registers in **Installed Apps**, migrates any legacy settings, and launches ready to use.
   - Optionally click **Add Desktop Shortcut** from the first-run banner or **Settings**.
3. Open **Settings** (top-right gear icon) to select your 4" x 6" thermal printer (e.g., `ZDesigner GK420d`) and print a test label.

### Auto-Updates & "What's New?"

When a newer version is published on GitHub Releases, an **Update available** banner appears cleanly at the top of the window (and in **Settings → Updates & Info**). Clicking **Update & Restart** downloads the matching architecture binary, verifies its SHA-256 digest against `checksums.txt`, atomically swaps the installed executable in `%LOCALAPPDATA%\QR Hub\`, restarts automatically with all Start Menu and Desktop shortcuts intact, and opens the **"What's New?"** release notes dialog.

### Command-Line Options

- `--portable` — Run in place without copying into `%LOCALAPPDATA%` or creating shortcuts.
- `--install` — Force self-installation to `%LOCALAPPDATA%\QR Hub\` and update shortcuts.
- `--uninstall` (add `--quiet` to skip prompts) — Remove shortcuts, Uninstall registry entry, and installed binary while preserving `%APPDATA%\QR Hub\config.json`.

## Building from Source

```bash
# Run unit tests
cargo test

# Native Windows build
cargo build --release

# Cross-compile from macOS with cargo-xwin + llvm + lld
cargo xwin build --release --target x86_64-pc-windows-msvc
cargo xwin build --release --cross-compiler clang --target aarch64-pc-windows-msvc
```
