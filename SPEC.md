# Specification: PostalAnnex+ Amazon QR Code Scanner (Windows / Rust Port)

## 1. Overview & Objectives

Build a standalone, native Windows desktop application in **Rust** that ports all business logic, branding, clipboard behaviors, and 4" x 6" thermal label printing from the legacy single-file web utility (`app.html` v1.3.2), while modernizing the counter experience for speed and reliability.

### Core Requirements

1. **Zero-Friction Scanning Homepage:** Opens to a clean PostalAnnex+ branded homepage displaying **"Ready to Scan"** with an **invisible text capture buffer** (no visible text box) that automatically clears stray keystrokes after 1 second of inactivity.

2. **QR Materialization Animation:** Plays a crisp animation when a QR code is generated so the code appears out of thin air / assembles itself from black blocks, without delaying printing or clipboard actions.

3. **100% Parity on Business & Clipboard Rules:** Retains all Caps Lock correction, 4-route return classification, interactive counter prompts, clipboard copy rules, raw data copy, override, and reprint capabilities.

4. **Silent Direct-to-Printer Thermal Output:** Prints immediately to a user-configured Windows printer at exact 4" x 6" dimensions via Win32 GDI with zero OS print dialogs.

5. **Dual-Architecture Windows Builds & Auto-Update:** Compiles native releases for both **Windows x86_64** and **Windows ARM64** via GitHub Actions, with built-in architecture-aware auto-updating and SHA-256 verification against GitHub Releases.

6. **Offline-First Reliability:** Scanning, QR generation, clipboard copy, and thermal printing must work 100% offline with zero external runtime dependencies (no WebView2, Electron, or bundled browser).

## 2. Target Platforms & Architectures

Every build and GitHub Release must target both Windows desktop architectures natively:

| Architecture | Rust Target Triple | Release Asset Filename | 
| ----- | ----- | ----- | 
| **Windows x64** | `x86_64-pc-windows-msvc` | `PostalAnnexQRScanner-windows-x64.exe` | 
| **Windows ARM64** | `aarch64-pc-windows-msvc` | `PostalAnnexQRScanner-windows-arm64.exe` | 

* Use `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` so no console window flashes when launching the release binary.

* Embed a Windows application icon and version metadata via `winres` (or `embed-resource`) in `build.rs`.

## 3. Recommended Tech Stack & Crates

Use a pure-Rust stack that produces a single self-contained `.exe` per architecture:

* **GUI Framework:** `eframe` / `egui` (fast startup, deterministic input event capture, custom `egui::Painter` support for the block-assembly QR animation).

* **QR Encoding:** `qrcode` crate (pure Rust, supports Error Correction Level `H`).

* **Windows Printing & Win32 APIs:** `windows` crate with features `Win32_Graphics_Printing`, `Win32_Graphics_Gdi`, `Win32_Foundation`, `Win32_Storage_Xps` (or direct GDI spooler calls).

* **Windows Clipboard:** `clipboard-win` (or `arboard`) wrapped in a retry helper (5 retries with 20 ms backoff) to handle transient `OpenClipboard` locks from PostalMate or Windows Clipboard History.

* **Config Persistence:** `serde`, `serde_json`, `directories` (stores config in `%APPDATA%\PostalAnnexQRScanner\config.json`).

* **Auto-Updater:** `reqwest` (with `rustls-tls` to avoid OpenSSL cross-compilation issues on ARM64), `semver`, `sha2`, `self-replace` (safely swaps the running Windows `.exe`).

## 4. Module Architecture

Decouple pure domain logic from GUI and Win32 I/O so all routing, normalization, and timer rules can be unit-tested cross-platform:

```
src/
├── main.rs              # Entry point, eframe initialization, window icon/geometry
├── domain/
│   ├── mod.rs
│   ├── parser.rs        # Caps Lock case-inversion engine, pipe splitting, 4-route classifier
│   ├── buffer.rs        # Invisible scan buffer with 1-second inactivity auto-clear state machine
│   └── state.rs         # Pure app state machine (Ready, AwaitingPrompt, LabelReady, Warning, Notice)
├── qr/
│   ├── mod.rs           # QR matrix generation (ECC Level H, boolean grid)
│   └── animation.rs     # Per-module staggered block-assembly animation state & math
├── clipboard.rs         # Windows clipboard writer with retry-on-lock
├── printer/
│   ├── mod.rs           # Printer traits, PrintJob, PrinterConfig
│   └── win32.rs         # EnumPrintersW, DEVMODEW 4" x 6" setup, DPI-exact GDI rendering
├── config.rs            # Load/save AppConfig JSON in %APPDATA%
├── updater.rs           # Arch-aware GitHub Releases checker, SHA-256 verifier, self-replace
└── ui/
    ├── mod.rs
    ├── app.rs           # Homepage UI, hidden input capture, prompts, QR animator, footer
    └── settings.rs      # Settings modal (printer picker, 4x6 tuning, test print, updates)

```

## 5. Business Logic, Input Engine & Routing Rules

### 5.1 Invisible Scan Capture & 1-Second Auto-Clear Engine

Instead of a visible `<input>` text box, the application maintains an **invisible keyboard/HID scan buffer** active whenever the main window is focused and the Settings modal is closed:

* **Event Capture:** Consume `egui::Event::Text(s)` and `egui::Event::Key { key: Key::Enter, pressed: true, .. }` at the root frame level (ignoring key events while any modal text input inside Settings has focus, and ignoring modifier shortcuts like `Ctrl+C`).

* **Why Window-Level Capture:** Unlike an invisible text widget that can lose focus when the associate clicks "Yes", "No", "Reprint", or empty card space, root-level key capture guarantees the scanner is always live as long as the app window is active.

* **1-Second Inactivity Auto-Clear Rule:**

  * Track `buffer: String` and `last_input_at: Option<Instant>`.

  * Whenever one or more printable characters are appended to `buffer`, record `last_input_at = Some(Instant::now())` and schedule a UI wakeup at $1000\text{ ms}$ (`ctx.request_repaint_after(Duration::from_millis(1000))`).

  * On every frame update, if `!buffer.is_empty()` and $\Delta t = \text{now} - \text{last\_input\_at} \ge 1.0\text{ s}$, immediately clear `buffer.clear()` and set `last_input_at = None`.

  * *Behavioral Result:* Because USB HID barcode/QR scanners transmit an entire payload at high speed (typically $5\text{--}50\text{ ms}$ between characters) ending with `Enter`, real scans complete well under the $1\text{ s}$ timeout, while accidental stray keystrokes automatically evaporate after 1 second without requiring a manual "Clear" button or red error box.

* **Submission Trigger (`Enter`):**

  * When `Enter` is pressed, take `buffer` (clearing `buffer` and `last_input_at` immediately).

  * If `text.trim().is_empty()`, ignore it and remain in the current state.

  * Otherwise, pass the raw string (preserving internal whitespace) to the Caps Lock engine and routing pipeline.

* **Optional Paste Support:** If `egui::Event::Paste(text)` is received on the main screen (and not in Settings), append it to `buffer` and update `last_input_at` so manual testing via `Ctrl+V` + `Enter` remains possible.

### 5.2 Caps Lock Case-Inversion Engine

Before routing, normalize `text` using the exact rule from `app.html`:

1. Compute `lower = text.to_lowercase()`.

2. If `(lower.starts_with("afnlf") || lower.starts_with("hr")) && (text.starts_with('a') || text.starts_with('h'))`:

   * Apply `flip_case(&text)`: invert the case of **every character individually** (uppercase becomes lowercase, lowercase becomes uppercase; digits/symbols unchanged).

   * *Critical:* Do **not** simply call `.to_uppercase()`, because mixed-case characters in the RMA or trailing QR payload must have their original casing restored.

### 5.3 State Flush on Every New Scan

On every non-empty `Enter` submission, immediately clear all previous scan state:

* Previous QR code preview and active animation state

* Print header text

* Alert badge and any custom badge color overrides

* Error/warning/notice banners

* Active confirmation prompts (`Customer Packed` / `Happy Return`) and their pending payload/RMA

* Happy Return note

* Raw scanned text display and `Copy All` button

* `Reprint Last Label` and `Force Print (Override)` buttons

* `last_rejected_text`

### 5.4 Four-Route Decision Matrix

Split normalized `text` on `|` into `sections`, where `first_section = sections[0]` and `rma = sections.get(1)` (optional). Evaluate in this **exact priority order**:

| Priority | Match Condition | Interactive Prompt | Clipboard Action | Print Header (Rendered Uppercase) | UI Status / Alert | Print Trigger | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **1** | `text.starts_with("HR")` | `"This appears to be a Happy Return, have you already tried accepting through PostalMate?"` (**No** / **Yes**) | **Yes:** Copy **ENTIRE** `text`  **No:** None | **Yes:** `"Happy Return"`  **No:** None | **Yes:** Yellow badge (`#ffc107` bg, `#212529` text): `"Full QR code copied.\n⚠ Please mark the auto-detect tracking number as UPS in PostalMate, NOT Happy Returns."`  **No:** Yellow note (`#fff3cd` bg, `#ffc107` border, `#856404` text): `"Please try this return in PostalMate first."` | **Yes:** Immediate background print  **No:** None | 
| **2** | `first_section.starts_with("AFNLFBF")` | None | If `rma` exists and is non-empty, copy `rma` (`sections[1]`) | `"No Box No Label"` | If `rma` is non-empty, green badge (`#28a745` bg, `#ffffff` text): `"Amazon QR Detected! RMA Copied: {rma}"` | Immediate background print | 
| **3** | `first_section.starts_with("AFNLF")` | `"This item is marked as customer packed. Has it been properly packed yet?"` (**No** / **Yes**) | **Yes:** If `rma` exists and is non-empty, copy `rma` (`sections[1]`)  **No:** None | **Yes:** `"Customer Packed"`  **No:** None | **Yes:** If `rma` is non-empty, green badge (`#28a745` bg, `#ffffff` text): `"Amazon QR Detected! RMA Copied: {rma}"`  **No:** Red notice (`#f8d7da` bg, `#f5c6cb` border, `#721c24` text) with blinking red `"NOTICE"`: `"Please charge customer for appropriate packing materials."` | **Yes:** Immediate background print  **No:** None | 
| **4** | Fallback (`else`) | None; displays `"Force Print (Override)"` button (`#ffc107` bg) | None (including on Override) | **Override:** Empty / Hidden (QR only, no header) | Yellow warning (`#fff3cd` bg, `#ffc107` border, `#856404` text) with blinking `"WARNING!"` (`#b58100`): `"Please check the return location of this QR code before accepting it."` | **Override click:** Immediate background print | 

### 5.5 Post-Generation Actions & Controls

Whenever a QR code is generated (Routes 1-Yes, 2, 3-Yes, and 4-Override):

* **QR Payload:** Always encode the **full normalized `text`** (never just the RMA substring).

* **Raw Data Box + Copy All:** Display a scrollable monospace container (`#e9ecef` bg, `#ced4da` border, `#495057` text) showing the full normalized `text`, with an overlaid `"Copy All"` button. Clicking `"Copy All"` copies the full raw text to the clipboard and switches the button styling to green (`#28a745` bg, white text) with label `"Copied!"` for $1200\text{ ms}$, then reverts.

* **Reprint Last Label:** Show a full-width `"Reprint Last Label"` button that re-dispatches the last generated `PrintJob` (same QR payload and header, including empty header for Override) without modifying the clipboard.

* **Return to Ready:** Even while displaying the last scanned QR label, the invisible scan buffer remains active and ready to accept the next scan immediately.

## 6. UI/UX Design, Branding & QR Materialization Animation

### 6.1 Homepage Layout & Branding

* **Window Title:** `PostalAnnex+ Amazon QR Code Scanner`

* **Default Window Size:** `460 x 720` logical pixels (resizable, centered on launch, background `#f0f2f5`).

* **Main Card:** Centered white card (`#ffffff`, rounded `12px` corners, subtle drop shadow) with a compact **Settings** gear icon button in the top-right corner of the card.

* **Brand Header (Top of Homepage):**

  * Italic bold logo: **`Postal`** (`#002f6c`) + **`Annex`** (`#d11226`) + **`+`** (`#002f6c`).

  * Subtext: Uppercase `"AMAZON QR CODE SCANNER"` (`#6c757d`, letter-spaced) with a `#002f6c` top rule separating it from the logo.

* **"Ready to Scan" Status Display (Replaces Text Input):**

  * Directly below the brand header, display a clean, prominent **"Ready to Scan"** status banner/pill (e.g., subtle soft-blue or neutral rounded container with a gently pulsing status indicator dot and bold `"Ready to Scan"` label in `#002f6c`).

  * No text input field, cursor, or Clear button is rendered on the homepage.

  * If no printer is configured yet, display a gentle amber setup notice below `"Ready to Scan"` with an `"Open Settings"` button.

* **Consistent Prompt Styling:** Both the `Customer Packed` and `Happy Return` confirmation panels must share identical card styling (`#f8f9fa` bg, `2px` `#002f6c` border, `8px` radius, bold `#002f6c` question text, equal-width side-by-side **No** `#dc3545` and **Yes** `#28a745` buttons). *(Note: This fixes the mismatched CSS class bug in `app.html`.)*

* **Counter Instructions:** Subtle dashed-top-border section near the bottom of the card:

  1. `Scan QR code while app shows "Ready to Scan"`

  2. `Label prints automatically to configured printer — place on item`

  3. `Press CTRL + V in PostalMate to automatically paste RMA number`

* **Bottom Version Footer (Very Bottom of Window/Page):**

  * Pinned cleanly at the very bottom of the page in small muted text (`10px`, `#adb5bd`): `Amazon Scanner v{CARGO_PKG_VERSION}` (optionally accompanied by a subtle small muted indicator of the active printer name, e.g., `Printer: ZDesigner GK420d` or `No Printer Configured`).

### 6.2 QR Code Materialization Animation ("Black Blocks from Nothingness")

When a QR code is generated on screen, render its dark modules via custom `egui::Painter` rectangles inside the `300 x 300` preview frame with a crisp, modern assembly animation:

* **Total Duration:** $T_{\text{total}} \approx 520\text{ ms}$ (snappy enough for retail counter speed, noticeable enough to feel tactile and rewarding).

* **Per-Module Timing:** For each dark module at grid coordinate $(r, c)$ in an $N \times N$ QR matrix:

  * Compute a deterministic pseudo-random hash $h(r, c) \in [0, 1)$ (e.g., SplitMix / FNV hash of `(r, c, scan_nonce)`) blended with normalized distance from the QR center $d(r, c) \in [0, 1]$:
    

    $$
    t_{\text{start}}(r, c) = \bigl(0.65 \cdot h(r, c) + 0.35 \cdot d(r, c)\bigr) \cdot 0.34\text{ s}
    $$

  * Each individual block animates over $\tau = 0.18\text{ s}$ with local progress:
    

    $$
    u = \text{clamp}\!\left(\frac{t - t_{\text{start}}(r, c)}{\tau},\; 0,\; 1\right)
    $$

  * Apply a cubic ease-out curve $e(u) = 1 - (1 - u)^3$:

    * **Scale:** Module grows from $0\%$ to $100\%$ of its cell size around its cell center.

    * **Micro-Assembly Drift:** Module starts displaced slightly outward along a deterministic micro-vector ($\approx 3\text{--}6\text{ px}$ scaled by $1 - e(u)$) and snaps crisply into its exact grid cell as $u \to 1$.

    * **Opacity:** Alpha fades from $0$ to $255$ as $e(u)$ increases, making black blocks materialize out of nothingness and lock together into the finished QR code.

* **Post-Animation State:** Once $t \ge T_{\text{total}}$, draw all dark modules at exact integer-snapped coordinates with pure `#000000` fill on `#ffffff` background and stop requesting continuous repaints (`0%` idle CPU usage).

* **Zero-Delay Printing Guarantee:** Trigger clipboard copy and dispatch the background Win32 print job **immediately** when the QR is generated — **never** wait for the screen animation to finish before printing.

### 6.3 Settings Modal & Persistence

Clicking the top-right gear icon opens a modal overlay (pausing invisible scan capture while open) backed by `%APPDATA%\PostalAnnexQRScanner\config.json`:

```
{
  "printer_name": "ZDesigner GK420d",
  "auto_print_on_scan": true,
  "force_4x6_devmode": true,
  "qr_size_inches": 3.125,
  "label_width_inches": 4.0,
  "label_height_inches": 6.0,
  "offset_x_mm": 0.0,
  "offset_y_mm": 0.0,
  "auto_check_updates": true,
  "github_repo": "KingDomino/Postal-Annex-QR-Hub"
}

```

* **Printer Selection:** Dropdown populated by `EnumPrintersW(PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS)`, with a **Refresh List** button and a **Test Print 4" x 6" Label** button (prints `"TEST LABEL"` + a test QR code to verify alignment immediately).

* **Layout Calibration:** Toggle for forcing 4" x 6" `DEVMODEW`, QR size slider ($2.0\text{"} \text{--} 3.5\text{"}$, default $3.125\text{"}$), and optional X/Y millimeter offset nudges for thermal printer feed variance.

* **Updates Section:** Shows current version, detected architecture (`x86_64` or `ARM64`), **"Check for Updates"** button, and **"Update & Restart"** button when a newer GitHub release is available.

## 7. QR Encoding & Direct Win32 4" x 6" Thermal Printing

### 7.1 QR Matrix Parameters

* **Error Correction Level:** `EcLevel::H` (High, 30% recovery).

* **Colors:** Pure black (`#000000`) dark modules on pure white (`#ffffff`) background.

* **Quiet Zone:** Minimum 4-module white margin around the QR matrix on both screen preview and printed label.

### 7.2 Win32 GDI Thermal Printing Pipeline

Replicate the `@media print` 4" x 6" centered thermal label layout directly through the Windows spooler without invoking any browser or print dialog:

1. **Printer DC & Paper Setup:**

   * Open the configured printer with `OpenPrinterW`.

   * Fetch and modify its `DEVMODEW` via `DocumentPropertiesW`.

   * When `force_4x6_devmode` is enabled, set `DM_PAPERSIZE | DM_PAPERWIDTH | DM_PAPERLENGTH | DM_ORIENTATION`, `dmPaperSize = DMPAPER_USER`, `dmPaperWidth = 1016` ($101.6\text{ mm} = 4.0\text{"}$), `dmPaperLength = 1524` ($152.4\text{ mm} = 6.0\text{"}$), and `dmOrientation = DMORIENT_PORTRAIT`, then validate via `DocumentPropertiesW(DM_IN_BUFFER | DM_OUT_BUFFER)`.

   * Create the device context via `CreateDCW("WINSPOOL", printer_name, None, Some(devmode_ptr))`.

2. **DPI-Exact Integer Module Rendering (Critical for 203 / 300 DPI Thermal Heads):**

   * Query `dpi_x = GetDeviceCaps(hdc, LOGPIXELSX)`, `dpi_y = GetDeviceCaps(hdc, LOGPIXELSY)`, `PHYSICALWIDTH`, `PHYSICALHEIGHT`, `PHYSICALOFFSETX`, and `PHYSICALOFFSETY`.

   * Compute target QR pixel width:
     

     $$
     W_{\text{target}} = \text{round}(\text{qr\_size\_inches} \cdot \text{dpi\_x})
     $$

   * Compute exact integer pixels per module (never use bilinear scaling, which causes grey/dithered edges on monochrome thermal printers):
     

     $$
     m = \max\!\left(1,\; \left\lfloor \frac{W_{\text{target}}}{N} \right\rfloor\right), \quad W_{\text{actual}} = m \cdot N
     $$

   * Draw each dark module with `FillRect` using a pure black brush (`RGB(0, 0, 0)`).

3. **Header Typography & Centering:**

   * When a header is present (`"NO BOX NO LABEL"`, `"CUSTOMER PACKED"`, `"HAPPY RETURN"`, or `"TEST LABEL"`), render it in **uppercase**, bold (`FW_BOLD`), sans-serif (`Segoe UI` / `Arial`), pure black, sized via `MulDiv(26, dpi_y, 72)`, centered horizontally above the QR code with a vertical gap of $\approx 0.21\text{"}$ (`round(0.21 * dpi_y)`).

   * Center the combined `[Header + Gap + QR]` block vertically and horizontally across the physical 4" x 6" label, applying any configured `offset_x_mm` / `offset_y_mm` nudges.

   * For **Force Print (Override)** jobs, omit the header and center the QR code alone.

4. **Thread & Failure Isolation:**

   * Run printing on a dedicated background worker thread so the UI and animation stay at 60 FPS.

   * Clipboard copy must execute independently and succeed even if printing fails (e.g., printer offline or not configured). Surface spooler failures as a clear, non-blocking red error toast with a button to open Settings.

## 8. Architecture-Aware GitHub Releases Auto-Updater & CI/CD

### 8.1 Runtime Auto-Updater

1. **Startup Check:** If `auto_check_updates` is `true`, wait $1\text{ s}$ after launch and query `GET https://api.github.com/repos/{github_repo}/releases/latest` on a background thread (`User-Agent: PostalAnnexQRScanner/{version}`, $5\text{ s}$ timeout). Fail silently on startup when offline; show status messages when triggered manually from Settings.

2. **SemVer Comparison:** Strip any leading `v` from `tag_name` and compare against `env!("CARGO_PKG_VERSION")` using `semver::Version`.

3. **Architecture-Specific Asset Selection:**

   ```
   #[cfg(target_arch = "x86_64")]
   const EXPECTED_ASSET: &str = "PostalAnnexQRScanner-windows-x64.exe";
   
   #[cfg(target_arch = "aarch64")]
   const EXPECTED_ASSET: &str = "PostalAnnexQRScanner-windows-arm64.exe";
   
   ```

4. **Non-Intrusive Prompt:** Display a compact banner at the top of the UI (`"Update available: v{new_version}"` + `"Update & Restart"` button). Never interrupt an active scan or open confirmation prompt automatically.

5. **Download, Verify & Swap:**

   * Download `EXPECTED_ASSET` and `checksums.txt` into `%TEMP%` with progress indication.

   * Parse `checksums.txt` for `EXPECTED_ASSET`, compute the downloaded binary's SHA-256 digest with `sha2`, and abort if the hash does not match.

   * Call `self_replace::self_replace(&temp_exe_path)` to atomically replace the running executable on Windows, relaunch via `std::process::Command::new(std::env::current_exe()?)`, and exit cleanly.

### 8.2 GitHub Actions Release Workflow (`.github/workflows/release.yml`)

Trigger on pushed tags matching `v*` (plus `workflow_dispatch` for manual test builds):

* **Build Matrix:**

  * `x86_64-pc-windows-msvc` on `windows-latest` -> produces `PostalAnnexQRScanner-windows-x64.exe`.

  * `aarch64-pc-windows-msvc` on `windows-11-arm` (or cross-compiled on `windows-latest` with `rustup target add aarch64-pc-windows-msvc` and MSVC ARM64 build tools) -> produces `PostalAnnexQRScanner-windows-arm64.exe`.

* **Test Gate:** Run `cargo test` before packaging release artifacts.

* **Checksums & Publish Job:**

  * Download both built `.exe` artifacts into a release staging directory.

  * Generate a combined `checksums.txt` containing lowercase SHA-256 hashes for both `PostalAnnexQRScanner-windows-x64.exe` and `PostalAnnexQRScanner-windows-arm64.exe` (`<sha256>  <filename>`).

  * Create the GitHub Release via `softprops/action-gh-release` attaching:

    * `PostalAnnexQRScanner-windows-x64.exe`

    * `PostalAnnexQRScanner-windows-arm64.exe`

    * `checksums.txt`

## 9. Verification & Acceptance Criteria

### 9.1 Automated Unit Tests (Required Before Completion)

1. **Invisible Buffer & 1-Second Auto-Clear Tests:**

   * Fast burst of characters arriving $< 100\text{ ms}$ apart followed by `Enter` submits the full string and clears the buffer.

   * Single stray character (or partial input) with no follow-up character for $\ge 1000\text{ ms}$ auto-clears the buffer so a subsequent real scan is completely uncorrupted.

   * Characters spaced $900\text{ ms}$ apart keep the buffer alive; pausing $1000\text{ ms}$ after the last character clears it.

   * Empty or whitespace-only `Enter` is ignored.

2. **Caps Lock Inversion Tests:**

   * `afnlfbf|rMa123|xYz` -> `AFNLFBF|RmA123|XyZ` -> **No Box No Label**, RMA `RmA123`.

   * `hr|AbC999` -> `HR|aBc999` -> **Happy Return**.

   * `AFNLFBF|AbC` -> unchanged.

   * `unknown|test` -> NOT flipped -> **Unrecognized Warning**.

3. **Routing & Clipboard Tests:**

   * `AFNLFBF|RMA12345|EXTRA` -> immediate print, header `"No Box No Label"`, clipboard `"RMA12345"`.

   * `AFNLFBF` (no pipe / missing RMA) -> immediate print, header `"No Box No Label"`, NO clipboard write, NO alert badge.

   * `AFNLF|RMA999|EXTRA` -> Customer Packed prompt; **Yes** copies `"RMA999"`, prints `"Customer Packed"`, green badge; **No** shows red packing materials notice, no copy, no print.

   * `HR|12345|DATA` -> Happy Return prompt; **Yes** copies **full** `"HR|12345|DATA"`, prints `"Happy Return"`, yellow UPS warning badge; **No** shows `"Please try this return in PostalMate first."`, no copy, no print.

   * `RANDOM_QR_DATA` -> yellow warning + Override button; **Override** prints QR with **no header** and performs **no clipboard write**.

4. **State & Animation Tests:**

   * Scanning a new code while a confirmation prompt is open flushes the old prompt and processes the new scan cleanly.

   * `Reprint` re-issues the last `PrintJob` (including headerless Override) without altering the clipboard.

   * `Copy All` copies full normalized payload regardless of whether auto-copy only copied the RMA.

   * QR animation transitions monotonically from $t = 0$ (all blocks hidden/unscaled) to $t \ge T_{\text{total}}$ (all dark modules at scale $1.0$, offset $0$, alpha $255$).

5. **Updater Checksum & Asset Matching Tests:**

   * Correctly selects `windows-x64.exe` on `x86_64` and `windows-arm64.exe` on `aarch64`.

   * Rejects update when SHA-256 does not match `checksums.txt`.

### 9.2 Windows Hardware Smoke Test Checklist

* Launch on Windows x64 and Windows ARM64: verify clean homepage with PostalAnnex+ logo, **"Ready to Scan"** banner (no text input box), and small version number at the very bottom.

* Type a stray letter (`x`), wait 1.5 seconds, then scan an `AFNLFBF|...` QR code: verify the stray `x` was auto-cleared and the scan routes cleanly without corruption.

* Verify the QR code plays the black-block assembly animation on screen while the thermal printer begins printing immediately with zero OS print dialogs.

* Verify crisp, unscattered black QR modules on a 4" x 6" thermal label and scan it back with the counter's handheld scanner.

* Verify `Ctrl+V` in PostalMate pastes the exact expected RMA (or full string for Happy Returns).

* Publish a test tag (`v0.1.1`) to GitHub Releases and verify both x64 and ARM64 binaries self-update cleanly to their matching architecture builds.