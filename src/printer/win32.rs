//! Direct Win32 GDI 4" x 6" thermal label printing pipeline (`#[cfg(windows)]`).

use std::ptr;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, FillRect,
    GetDeviceCaps, GetTextExtentPoint32W, SelectObject, SetBkMode, SetTextColor,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_QUALITY, DEVMODEW, DMORIENT_PORTRAIT,
    DMPAPER_USER, DM_FORMNAME, DM_IN_BUFFER, DM_ORIENTATION, DM_OUT_BUFFER, DM_PAPERLENGTH,
    DM_PAPERSIZE, DM_PAPERWIDTH, DT_CENTER, DT_NOCLIP, DT_NOPREFIX, DT_SINGLELINE, DT_TOP,
    FF_SWISS, FW_BOLD, HBRUSH, HDC, HGDIOBJ, LOGPIXELSX, LOGPIXELSY, OUT_TT_PRECIS, PHYSICALHEIGHT,
    PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, TRANSPARENT, VARIABLE_PITCH,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, OpenPrinterW, PRINTER_ENUM_CONNECTIONS,
    PRINTER_ENUM_LOCAL, PRINTER_INFO_4W,
};
use windows::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};

use crate::config::AppConfig;
use crate::domain::PrintJob;
use crate::printer::{compute_label_layout, DeviceMetrics, LabelLayout};
use crate::qr;
use crate::wide::to_wide_null;

/// Safely reads a null-terminated UTF-16 pointer into a Rust `String`.
unsafe fn pwstr_to_string(ptr: PWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *ptr.0.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(ptr.0, len);
    Some(String::from_utf16_lossy(slice))
}

/// Zeroed, 8-byte-aligned byte buffer for Win32 APIs that fill variable-size structs
/// (`PRINTER_INFO_4W[]`, `DEVMODEW` + driver data). A plain `Vec<u8>` is only 1-byte
/// aligned, so casting it to those struct types would be undefined behavior.
struct AlignedBuf {
    words: Vec<u64>,
    len: usize,
}

impl AlignedBuf {
    fn new(len: usize) -> Self {
        Self {
            words: vec![0u64; len.div_ceil(std::mem::size_of::<u64>())],
            len,
        }
    }

    fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.words.as_mut_ptr().cast::<u8>(), self.len) }
    }

    fn as_mut_ptr<T>(&mut self) -> *mut T {
        const { assert!(std::mem::align_of::<T>() <= std::mem::align_of::<u64>()) };
        self.words.as_mut_ptr().cast::<T>()
    }

    fn len(&self) -> usize {
        self.len
    }
}

struct PrinterHandleGuard(HANDLE);

impl Drop for PrinterHandleGuard {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = ClosePrinter(self.0);
            }
        }
    }
}

struct DcGuard(HDC);

impl Drop for DcGuard {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
}

struct GdiObjectGuard(HGDIOBJ);

impl Drop for GdiObjectGuard {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

struct SelectedObjectGuard {
    hdc: HDC,
    prev: HGDIOBJ,
}

impl Drop for SelectedObjectGuard {
    fn drop(&mut self) {
        if is_valid_gdi_obj(self.prev) {
            unsafe {
                let _ = SelectObject(self.hdc, self.prev);
            }
        }
    }
}

struct DocGuard {
    hdc: HDC,
    finished: bool,
}

impl DocGuard {
    fn finish(mut self) -> Result<(), String> {
        let rc = unsafe { EndDoc(self.hdc) };
        self.finished = true;
        if rc <= 0 {
            Err("Win32 EndDoc failed while finalizing print job.".to_string())
        } else {
            Ok(())
        }
    }
}

impl Drop for DocGuard {
    fn drop(&mut self) {
        if !self.finished && !self.hdc.0.is_null() {
            unsafe {
                let _ = AbortDoc(self.hdc);
            }
        }
    }
}

/// Enumerates local and connected Windows printers using `EnumPrintersW`.
pub fn enumerate_printers() -> Result<Vec<String>, String> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed: u32 = 0;
    let mut returned: u32 = 0;

    unsafe {
        let _ = EnumPrintersW(flags, PCWSTR::null(), 4, None, &mut needed, &mut returned);
    }

    if needed == 0 {
        return Ok(Vec::new());
    }

    let mut buf = AlignedBuf::new(needed as usize);
    let ok = unsafe {
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            4,
            Some(buf.as_bytes_mut()),
            &mut needed,
            &mut returned,
        )
    };
    if ok.is_err() {
        return Err("Failed to enumerate installed Windows printers.".to_string());
    }

    let fits =
        (returned as usize).saturating_mul(std::mem::size_of::<PRINTER_INFO_4W>()) <= buf.len();
    if !fits {
        return Err("Printer list returned by Windows was larger than its buffer.".to_string());
    }

    let mut names = Vec::with_capacity(returned as usize);
    let infos = buf.as_mut_ptr::<PRINTER_INFO_4W>() as *const PRINTER_INFO_4W;
    for i in 0..(returned as usize) {
        let info = unsafe { &*infos.add(i) };
        if let Some(name) = unsafe { pwstr_to_string(info.pPrinterName) } {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                names.push(trimmed.to_string());
            }
        }
    }

    names.sort();
    names.dedup();
    Ok(names)
}

/// Prints a QR label directly through the Windows GDI spooler with zero OS dialogs.
pub fn print_label_gdi(job: &PrintJob, config: &AppConfig) -> Result<(), String> {
    let printer_name = config.active_printer().ok_or_else(|| {
        "No printer configured. Open Settings to select a 4\" x 6\" thermal printer.".to_string()
    })?;

    let matrix = qr::encode(&job.payload)?;
    let header_upper = job
        .header
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_uppercase);

    let printer_wide = to_wide_null(printer_name);
    let mut printer_handle = HANDLE::default();

    unsafe {
        OpenPrinterW(PCWSTR(printer_wide.as_ptr()), &mut printer_handle, None)
            .map_err(|e| format!("Failed to open printer \"{printer_name}\": {e}"))?;
    }
    let _printer_guard = PrinterHandleGuard(printer_handle);

    // Fetch and optionally configure a 4" x 6" portrait DEVMODEW via DocumentPropertiesW.
    let mut devmode_buf = prepare_devmode(printer_handle, &printer_wide, config)?;
    let devmode_ptr = devmode_buf
        .as_mut()
        .map(|b| b.as_mut_ptr::<DEVMODEW>() as *const DEVMODEW);

    let driver_wide = to_wide_null("WINSPOOL");
    let hdc = unsafe {
        CreateDCW(
            PCWSTR(driver_wide.as_ptr()),
            PCWSTR(printer_wide.as_ptr()),
            PCWSTR::null(),
            devmode_ptr,
        )
    };
    if hdc.0.is_null() {
        return Err(format!(
            "Failed to create Win32 GDI device context for printer \"{printer_name}\"."
        ));
    }
    let _dc_guard = DcGuard(hdc);

    let metrics = unsafe {
        DeviceMetrics {
            dpi_x: GetDeviceCaps(hdc, LOGPIXELSX),
            dpi_y: GetDeviceCaps(hdc, LOGPIXELSY),
            physical_width: GetDeviceCaps(hdc, PHYSICALWIDTH),
            physical_height: GetDeviceCaps(hdc, PHYSICALHEIGHT),
            physical_offset_x: GetDeviceCaps(hdc, PHYSICALOFFSETX),
            physical_offset_y: GetDeviceCaps(hdc, PHYSICALOFFSETY),
        }
    };

    let doc_name_wide = to_wide_null("PostalAnnex+ QR Label");
    let doc_info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(doc_name_wide.as_ptr()),
        lpszOutput: PCWSTR::null(),
        lpszDatatype: PCWSTR::null(),
        fwType: 0,
    };

    let job_id = unsafe { StartDocW(hdc, &doc_info) };
    if job_id <= 0 {
        return Err(format!(
            "Failed to start print job on printer \"{printer_name}\"."
        ));
    }
    let doc_guard = DocGuard {
        hdc,
        finished: false,
    };

    if unsafe { StartPage(hdc) } <= 0 {
        return Err(format!(
            "Failed to start page on printer \"{printer_name}\"."
        ));
    }

    let max_header_w = ((metrics.physical_width.max(1) as f32) * 0.90).round() as i32;
    let mut header_wide: Vec<u16> = header_upper
        .as_deref()
        .map(|h| h.encode_utf16().collect())
        .unwrap_or_default();
    let header_font = match header_upper {
        Some(_) => Some(fit_header_font(
            hdc,
            &header_wide,
            max_header_w,
            metrics.dpi_y.max(72),
        )?),
        None => None,
    };

    let layout = compute_label_layout(
        metrics,
        matrix.width,
        header_font.is_some(),
        header_font.as_ref().map_or(0, |f| f.text_height),
        config,
    );

    if let Some(font) = &header_font {
        draw_header(hdc, &mut header_wide, &layout, metrics, font.font_height);
    }
    draw_qr_modules(hdc, &matrix, &layout)?;

    if unsafe { EndPage(hdc) } <= 0 {
        return Err(format!(
            "Failed to finish page on printer \"{printer_name}\"."
        ));
    }

    doc_guard.finish()
}

/// Point size of the label header before any shrink-to-fit.
const HEADER_FONT_PT: i32 = 26;
/// Smallest point size a long header may shrink to.
const MIN_HEADER_FONT_PT: i32 = 10;

/// `SelectObject` signals failure with either NULL or `HGDI_ERROR` (-1).
fn is_valid_gdi_obj(obj: HGDIOBJ) -> bool {
    !obj.0.is_null() && obj.0 as isize != -1
}

/// Header font selected into the DC. Field order matters: the selection is restored
/// before the font is deleted.
struct HeaderFont {
    _selection: SelectedObjectGuard,
    _font: GdiObjectGuard,
    /// Measured text height in device pixels.
    text_height: i32,
    /// Font height passed to `CreateFontW` (negative = character height).
    font_height: i32,
}

fn create_bold_font(face: &[u16], height_px: i32) -> Result<GdiObjectGuard, String> {
    let font = unsafe {
        CreateFontW(
            height_px,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            DEFAULT_QUALITY.0 as u32,
            (VARIABLE_PITCH.0 | FF_SWISS.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    };
    if font.0.is_null() {
        Err("Failed to create Win32 header font.".to_string())
    } else {
        Ok(GdiObjectGuard(HGDIOBJ(font.0)))
    }
}

/// Selects a bold Segoe UI header font, shrinking it (down to 10 pt) when `text` is wider
/// than `max_width` (e.g. long 1Z tracking or UNPK headers).
fn fit_header_font(
    hdc: HDC,
    text: &[u16],
    max_width: i32,
    dpi_y: i32,
) -> Result<HeaderFont, String> {
    let face = to_wide_null("Segoe UI");
    let mut font_height = -mul_div_i32(HEADER_FONT_PT, dpi_y, 72);
    let mut font = create_bold_font(&face, font_height)?;
    let prev = unsafe { SelectObject(hdc, font.0) };
    let selection = SelectedObjectGuard { hdc, prev };

    let mut size = SIZE::default();
    let measured = unsafe { GetTextExtentPoint32W(hdc, text, &mut size).as_bool() };
    if measured && max_width > 0 && size.cx > max_width {
        let scaled_pt = ((HEADER_FONT_PT as f32 * max_width as f32) / size.cx as f32)
            .floor()
            .clamp(MIN_HEADER_FONT_PT as f32, HEADER_FONT_PT as f32) as i32;
        if scaled_pt < HEADER_FONT_PT {
            let scaled_height = -mul_div_i32(scaled_pt, dpi_y, 72);
            let scaled = create_bold_font(&face, scaled_height)?;
            // Only swap fonts if the selection worked: deleting a still-selected font leaks it.
            if is_valid_gdi_obj(unsafe { SelectObject(hdc, scaled.0) }) {
                unsafe {
                    let _ = GetTextExtentPoint32W(hdc, text, &mut size);
                }
                font_height = scaled_height;
                font = scaled;
            }
        }
    }

    Ok(HeaderFont {
        _selection: selection,
        _font: font,
        text_height: if size.cy > 0 {
            size.cy
        } else {
            font_height.abs()
        },
        font_height,
    })
}

/// Draws the centered uppercase header above the QR code in pure black.
fn draw_header(
    hdc: HDC,
    text: &mut [u16],
    layout: &LabelLayout,
    metrics: DeviceMetrics,
    font_height: i32,
) {
    unsafe {
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(0x0000_0000));
    }
    let half_w = (metrics.physical_width.max(layout.qr_actual_px) / 2).max(layout.qr_actual_px);
    let mut rect = RECT {
        left: layout.header_center_x_px - half_w,
        top: layout.header_y_px,
        right: layout.header_center_x_px + half_w,
        bottom: layout.header_y_px + layout.header_height_px.max(font_height.abs()) + 8,
    };
    unsafe {
        let _ = DrawTextW(
            hdc,
            text,
            &mut rect,
            // NOPREFIX: print '&' literally instead of as an accelerator underline.
            DT_CENTER | DT_TOP | DT_SINGLELINE | DT_NOCLIP | DT_NOPREFIX,
        );
    }
}

/// Fills every dark QR module as a pure black rect at exact integer device coordinates.
fn draw_qr_modules(hdc: HDC, matrix: &qr::QrMatrix, layout: &LabelLayout) -> Result<(), String> {
    let brush: HBRUSH = unsafe { CreateSolidBrush(COLORREF(0x0000_0000)) };
    if brush.0.is_null() {
        return Err("Failed to create Win32 black brush.".to_string());
    }
    let _brush_guard = GdiObjectGuard(HGDIOBJ(brush.0));

    let m = layout.module_px;
    for (row, col) in matrix.dark_modules() {
        let left = layout.qr_origin_x_px + (col as i32) * m;
        let top = layout.qr_origin_y_px + (row as i32) * m;
        let rect = RECT {
            left,
            top,
            right: left + m,
            bottom: top + m,
        };
        if unsafe { FillRect(hdc, &rect, brush) } == 0 {
            return Err("Win32 FillRect failed while drawing QR module.".to_string());
        }
    }
    Ok(())
}

/// Allocates, populates, and validates a `DEVMODEW` buffer for the target printer.
fn prepare_devmode(
    printer_handle: HANDLE,
    printer_wide: &[u16],
    config: &AppConfig,
) -> Result<Option<AlignedBuf>, String> {
    let needed = unsafe {
        DocumentPropertiesW(
            HWND(ptr::null_mut()),
            printer_handle,
            PCWSTR(printer_wide.as_ptr()),
            None,
            None,
            0,
        )
    };
    // A buffer smaller than DEVMODEW would make the field writes below out of bounds.
    if needed <= 0 || (needed as usize) < std::mem::size_of::<DEVMODEW>() {
        return Ok(None);
    }

    let mut buf = AlignedBuf::new(needed as usize);
    let dm_ptr = buf.as_mut_ptr::<DEVMODEW>();

    let rc = unsafe {
        DocumentPropertiesW(
            HWND(ptr::null_mut()),
            printer_handle,
            PCWSTR(printer_wide.as_ptr()),
            Some(dm_ptr),
            None,
            DM_OUT_BUFFER.0,
        )
    };
    if rc < 0 {
        return Ok(None);
    }

    if config.force_4x6_devmode {
        unsafe {
            let dm = &mut *dm_ptr;
            // 1 inch = 25.4 mm = 254 tenths of a millimeter (4.0" -> 1016, 6.0" -> 1524).
            let width_tenths_mm =
                (config.label_width_inches.clamp(1.0, 12.0) * 254.0).round() as i16;
            let height_tenths_mm =
                (config.label_height_inches.clamp(1.0, 18.0) * 254.0).round() as i16;

            dm.dmFields &= !DM_FORMNAME;
            dm.dmFields |= DM_ORIENTATION | DM_PAPERSIZE | DM_PAPERWIDTH | DM_PAPERLENGTH;
            dm.Anonymous1.Anonymous1.dmOrientation = DMORIENT_PORTRAIT as i16;
            dm.Anonymous1.Anonymous1.dmPaperSize = DMPAPER_USER as i16;
            dm.Anonymous1.Anonymous1.dmPaperWidth = width_tenths_mm;
            dm.Anonymous1.Anonymous1.dmPaperLength = height_tenths_mm;

            // Validate our modified DEVMODEW with the printer driver.
            let _ = DocumentPropertiesW(
                HWND(ptr::null_mut()),
                printer_handle,
                PCWSTR(printer_wide.as_ptr()),
                Some(dm_ptr),
                Some(dm_ptr as *const DEVMODEW),
                (DM_IN_BUFFER | DM_OUT_BUFFER).0,
            );
        }
    }

    Ok(Some(buf))
}

#[inline]
fn mul_div_i32(number: i32, numerator: i32, denominator: i32) -> i32 {
    if denominator == 0 {
        return 0;
    }
    let prod = (number as i64) * (numerator as i64);
    let denom = denominator as i64;
    if prod >= 0 {
        ((prod + denom / 2) / denom) as i32
    } else {
        ((prod - denom / 2) / denom) as i32
    }
}
