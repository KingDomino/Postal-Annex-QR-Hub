//! Native `eframe` / `egui` user interface for QR Hub (PostalAnnex+ Amazon QR Code Scanner).
//!
//! - [`app`]: homepage (invisible scan capture, prompts, QR materialization, footer).
//! - [`chrome`]: seamless borderless window titlebar, controls, and 8-way resize frame.
//! - [`settings`]: modal settings overlay (printer, 4" x 6" calibration, updates).
//! - [`whats_new`]: post-update "What's New?" changelog modal with live GitHub mirroring.
//!
//! This module also hosts the shared brand palette, font setup and small custom-painted
//! widgets. Every clickable homepage control is created with `Sense::CLICK` only (never
//! `FOCUSABLE`), so the USB HID scanner's trailing `Enter` can never "press" a button.

mod app;
mod chrome;
mod settings;
mod whats_new;

pub use app::ScannerApp;

use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Frame,
    InnerResponse, Margin, Painter, Pos2, Rect, Response, Sense, Shadow, Shape, Stroke, StrokeKind,
    TextFormat, Ui, Vec2,
};

/// Brand and state palette (mirrors `app.html` / `SPEC.md`).
pub(crate) mod colors {
    use eframe::egui::Color32;

    pub const WINDOW_BG: Color32 = Color32::from_rgb(0xf0, 0xf2, 0xf5);
    pub const WHITE: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
    pub const CARD_STROKE: Color32 = Color32::from_rgb(0xe3, 0xe6, 0xea);

    pub const NAVY: Color32 = Color32::from_rgb(0x00, 0x2f, 0x6c);
    pub const NAVY_HOVER: Color32 = Color32::from_rgb(0x00, 0x24, 0x54);
    pub const BRAND_RED: Color32 = Color32::from_rgb(0xd1, 0x12, 0x26);

    pub const MUTED: Color32 = Color32::from_rgb(0x6c, 0x75, 0x7d);
    pub const FOOTER: Color32 = Color32::from_rgb(0xad, 0xb5, 0xbd);
    pub const TEXT_DARK: Color32 = Color32::from_rgb(0x21, 0x25, 0x29);
    pub const TEXT_BODY: Color32 = Color32::from_rgb(0x49, 0x50, 0x57);

    pub const GRAY_100: Color32 = Color32::from_rgb(0xf8, 0xf9, 0xfa);
    pub const GRAY_150: Color32 = Color32::from_rgb(0xf1, 0xf3, 0xf5);
    pub const GRAY_200: Color32 = Color32::from_rgb(0xe9, 0xec, 0xef);
    pub const GRAY_300: Color32 = Color32::from_rgb(0xde, 0xe2, 0xe6);
    pub const GRAY_400: Color32 = Color32::from_rgb(0xce, 0xd4, 0xda);

    pub const READY_BG: Color32 = Color32::from_rgb(0xee, 0xf4, 0xfb);
    pub const READY_BORDER: Color32 = Color32::from_rgb(0xc2, 0xd6, 0xee);

    pub const GREEN: Color32 = Color32::from_rgb(0x28, 0xa7, 0x45);
    pub const GREEN_HOVER: Color32 = Color32::from_rgb(0x21, 0x88, 0x38);
    pub const RED: Color32 = Color32::from_rgb(0xdc, 0x35, 0x45);
    pub const RED_HOVER: Color32 = Color32::from_rgb(0xc8, 0x23, 0x33);

    pub const YELLOW: Color32 = Color32::from_rgb(0xff, 0xc1, 0x07);
    pub const YELLOW_HOVER: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x00);
    pub const AMBER_TEXT: Color32 = Color32::from_rgb(0xb5, 0x81, 0x00);
    pub const NOTE_BG: Color32 = Color32::from_rgb(0xff, 0xf3, 0xcd);
    pub const NOTE_TEXT: Color32 = Color32::from_rgb(0x85, 0x64, 0x04);

    pub const DANGER_BG: Color32 = Color32::from_rgb(0xf8, 0xd7, 0xda);
    pub const DANGER_BORDER: Color32 = Color32::from_rgb(0xf5, 0xc6, 0xcb);
    pub const DANGER_TEXT: Color32 = Color32::from_rgb(0x72, 0x1c, 0x24);

    pub const UPDATE_BG: Color32 = Color32::from_rgb(0xe7, 0xef, 0xf8);
    pub const UPDATE_BORDER: Color32 = Color32::from_rgb(0xb6, 0xcb, 0xe4);
    pub const UPDATE_TEXT: Color32 = Color32::from_rgb(0x1f, 0x3a, 0x5f);
}

// ---------------------------------------------------------------------------
// Layout constants & pure helpers
// ---------------------------------------------------------------------------

/// Maximum width of the white homepage card (inside the default 460 px window).
pub(crate) const CARD_MAX_WIDTH: f32 = 404.0;
/// Width of centered modal dialogs (Settings and "What's New?").
pub(crate) const MODAL_WIDTH: f32 = 390.0;
/// Minimum horizontal breathing room on each side of the card.
pub(crate) const CARD_MIN_SIDE_MARGIN: f32 = 14.0;
/// Edge length of the on-screen QR preview frame.
pub(crate) const QR_PREVIEW_SIZE: f32 = 300.0;
/// CSS `@keyframes blink` period (`1.2s`, visible for the first half).
pub(crate) const BLINK_PERIOD_MS: u64 = 1200;
/// How long the "Copied!" confirmation stays green.
pub(crate) const COPIED_FEEDBACK: Duration = Duration::from_millis(1200);
/// Debounce delay before persisting settings changes to `config.json`.
pub(crate) const CONFIG_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Returns `(visible, time_until_next_toggle)` for the 1.2 s NOTICE / WARNING blink cycle.
pub(crate) fn blink_phase(elapsed: Duration) -> (bool, Duration) {
    let half = BLINK_PERIOD_MS / 2;
    let ms = (elapsed.as_millis() % u128::from(BLINK_PERIOD_MS)) as u64;
    let visible = ms < half;
    let until_toggle = half - (ms % half);
    (visible, Duration::from_millis(until_toggle.max(1)))
}

/// Snaps a logical coordinate onto the physical pixel grid.
pub(crate) fn snap_to_pixel(value: f32, pixels_per_point: f32) -> f32 {
    if pixels_per_point <= 0.0 {
        return value;
    }
    (value * pixels_per_point).round() / pixels_per_point
}

/// Card width for a given available panel width.
pub(crate) fn card_width(available: f32) -> f32 {
    (available - 2.0 * CARD_MIN_SIDE_MARGIN)
        .min(CARD_MAX_WIDTH)
        .max(0.0)
}

/// Formats a byte count as a short human-readable string.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MIB {
        format!("{:.1} MB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

// ---------------------------------------------------------------------------
// Fonts & theme
// ---------------------------------------------------------------------------

const FAMILY_SEMIBOLD: &str = "pa-semibold";
const FAMILY_BOLD: &str = "pa-bold";
const FAMILY_BOLD_ITALIC: &str = "pa-bold-italic";

const REGULAR_FILES: &[&str] = &["segoeui.ttf", "Arial.ttf"];
const SEMIBOLD_FILES: &[&str] = &["seguisb.ttf", "Arial Bold.ttf"];
const BOLD_FILES: &[&str] = &["segoeuib.ttf", "Arial Bold.ttf"];
const BOLD_ITALIC_FILES: &[&str] = &["segoeuiz.ttf", "Arial Bold Italic.ttf"];

/// Which optional system fonts were found at startup.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FontSupport {
    /// A real bold-italic face is loaded (otherwise the logo uses egui's synthetic italics).
    pub true_bold_italic: bool,
}

fn system_font_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![windir.join("Fonts")]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/System/Library/Fonts/Supplemental"),
            PathBuf::from("/Library/Fonts"),
        ]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Cheap sanity check that `bytes` is a single TrueType/OpenType font (not a collection),
/// so a corrupt or unexpected file can never make `egui` panic while parsing it.
fn looks_like_single_font(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(0..4),
        Some([0x00, 0x01, 0x00, 0x00]) | Some(b"true") | Some(b"OTTO")
    ) && bytes.len() > 1024
}

fn load_system_font(
    defs: &mut FontDefinitions,
    dirs: &[PathBuf],
    key: &str,
    files: &[&str],
) -> Option<String> {
    for dir in dirs {
        for file in files {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                if looks_like_single_font(&bytes) {
                    defs.font_data
                        .insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
                    return Some(key.to_owned());
                }
            }
        }
    }
    None
}

/// Installs fonts: crisp system faces (Segoe UI on Windows) when available, always backed
/// by egui's bundled fonts (which also provide `⚠`, `•`, `✓` glyph fallbacks).
/// The named families used by this UI are ALWAYS registered, so lookups never panic.
fn install_fonts(ctx: &egui::Context) -> FontSupport {
    let mut defs = FontDefinitions::default();
    let dirs = system_font_dirs();

    let regular = load_system_font(&mut defs, &dirs, "sys-regular", REGULAR_FILES);
    let semibold = load_system_font(&mut defs, &dirs, "sys-semibold", SEMIBOLD_FILES);
    let bold = load_system_font(&mut defs, &dirs, "sys-bold", BOLD_FILES);
    let bold_italic = load_system_font(&mut defs, &dirs, "sys-bold-italic", BOLD_ITALIC_FILES);

    let mut proportional = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(key) = &regular {
        proportional.insert(0, key.clone());
    }
    defs.families
        .insert(FontFamily::Proportional, proportional.clone());

    if let Some(key) = &regular {
        // Keep Hack as the primary monospace face; Segoe UI only as a glyph fallback.
        if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
            mono.push(key.clone());
        }
    }

    let with_primary = |primary: Option<&String>| {
        let mut list = proportional.clone();
        if let Some(key) = primary {
            list.insert(0, key.clone());
        }
        list
    };
    let semibold_list = with_primary(semibold.as_ref().or(bold.as_ref()));
    let bold_list = with_primary(bold.as_ref());
    let bold_italic_list = with_primary(bold_italic.as_ref().or(bold.as_ref()));

    defs.families
        .insert(FontFamily::Name(FAMILY_SEMIBOLD.into()), semibold_list);
    defs.families
        .insert(FontFamily::Name(FAMILY_BOLD.into()), bold_list);
    defs.families
        .insert(FontFamily::Name(FAMILY_BOLD_ITALIC.into()), bold_italic_list);

    ctx.set_fonts(defs);

    FontSupport {
        true_bold_italic: bold_italic.is_some(),
    }
}

/// Forces the clean light theme, brand colors, crisp rounding and spacing, and fonts.
pub(crate) fn install_theme(ctx: &egui::Context) -> FontSupport {
    let fonts = install_fonts(ctx);

    ctx.set_theme(egui::ThemePreference::Light);

    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = colors::WINDOW_BG;
    visuals.window_fill = colors::WHITE;
    visuals.extreme_bg_color = colors::WHITE;
    visuals.faint_bg_color = colors::GRAY_100;
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0_f32, colors::CARD_STROKE);
    visuals.selection.bg_fill = Color32::from_rgb(0xc9, 0xdb, 0xf2);
    visuals.selection.stroke = Stroke::new(1.0_f32, colors::NAVY);
    visuals.hyperlink_color = colors::NAVY;
    visuals.override_text_color = None;

    let radius = CornerRadius::same(6);
    let w = &mut visuals.widgets;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = radius;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, colors::GRAY_300);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, colors::TEXT_BODY);
    w.inactive.bg_fill = colors::GRAY_200;
    w.inactive.weak_bg_fill = colors::GRAY_200;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, colors::GRAY_400);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, colors::TEXT_DARK);
    w.hovered.bg_fill = colors::GRAY_300;
    w.hovered.weak_bg_fill = colors::GRAY_300;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, colors::NAVY);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, colors::TEXT_DARK);
    w.hovered.expansion = 0.0;
    w.active.bg_fill = colors::GRAY_400;
    w.active.weak_bg_fill = colors::GRAY_400;
    w.active.bg_stroke = Stroke::new(1.0_f32, colors::NAVY);
    w.active.fg_stroke = Stroke::new(1.5_f32, colors::TEXT_DARK);
    w.active.expansion = 0.0;

    ctx.set_visuals_of(egui::Theme::Light, visuals);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        use egui::{FontFamily::Monospace, FontFamily::Proportional, TextStyle};
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.slider_width = 170.0;
        style.spacing.combo_width = 220.0;
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(14.0, Proportional)),
            (TextStyle::Button, FontId::new(14.0, Proportional)),
            (TextStyle::Heading, FontId::new(18.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        ]
        .into();
        style.interaction.selectable_labels = false;
    });

    fonts
}

pub(crate) fn font_regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub(crate) fn font_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_SEMIBOLD.into()))
}

pub(crate) fn font_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BOLD.into()))
}

pub(crate) fn font_bold_italic(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BOLD_ITALIC.into()))
}

pub(crate) fn font_mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Text format helper for [`egui::text::LayoutJob`] sections.
pub(crate) fn text_format(font: FontId, color: Color32, letter_spacing: f32) -> TextFormat {
    TextFormat {
        font_id: font,
        color,
        extra_letter_spacing: letter_spacing,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Custom-painted widgets
// ---------------------------------------------------------------------------

/// Visual recipe for a flat, custom-painted button.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ButtonStyle {
    pub fill: Color32,
    pub hover_fill: Color32,
    pub stroke: Option<Color32>,
    pub text: Color32,
    pub radius: u8,
}

impl ButtonStyle {
    pub const fn solid(fill: Color32, hover_fill: Color32, text: Color32) -> Self {
        Self {
            fill,
            hover_fill,
            stroke: None,
            text,
            radius: 6,
        }
    }

    pub const fn with_stroke(mut self, stroke: Color32) -> Self {
        self.stroke = Some(stroke);
        self
    }

    pub const fn with_radius(mut self, radius: u8) -> Self {
        self.radius = radius;
        self
    }
}

/// Shared neutral gray button style used for secondary controls across the app.
pub(crate) const fn secondary_button_style() -> ButtonStyle {
    ButtonStyle::solid(colors::GRAY_200, colors::GRAY_300, colors::TEXT_BODY)
        .with_stroke(colors::GRAY_400)
}

/// Shared white rounded modal card frame with soft elevation shadow.
pub(crate) fn modal_frame() -> Frame {
    Frame::new()
        .fill(colors::WHITE)
        .stroke(Stroke::new(1.0_f32, colors::CARD_STROKE))
        .corner_radius(CornerRadius::same(12))
        .shadow(Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(64),
        })
        .inner_margin(Margin::same(20))
}

fn darken(color: Color32, factor: f32) -> Color32 {
    let f = factor.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (f32::from(color.r()) * f) as u8,
        (f32::from(color.g()) * f) as u8,
        (f32::from(color.b()) * f) as u8,
        color.a(),
    )
}

/// Paints a button's visuals into `rect` for an already-registered `response`.
pub(crate) fn paint_button(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    label: &str,
    font: FontId,
    style: ButtonStyle,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let fill = if response.is_pointer_button_down_on() {
        darken(style.hover_fill, 0.92)
    } else if response.hovered() {
        style.hover_fill
    } else {
        style.fill
    };
    let stroke = style
        .stroke
        .map_or(Stroke::NONE, |c| Stroke::new(1.0_f32, c));
    let painter = ui.painter();
    painter.rect(rect, style.radius, fill, stroke, StrokeKind::Inside);
    let galley = painter.layout(
        label.to_owned(),
        font,
        style.text,
        (rect.width() - 8.0).max(1.0),
    );
    let pos = rect.center() - galley.size() * 0.5;
    painter.galley(pos, galley, style.text);
}

/// Allocates and paints a non-focusable button of exactly `size`.
pub(crate) fn flat_button(
    ui: &mut Ui,
    size: Vec2,
    label: &str,
    font: FontId,
    style: ButtonStyle,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::CLICK);
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    paint_button(ui, rect, &response, label, font, style);
    response
}

/// Paints a non-focusable button at an absolute `rect` WITHOUT allocating layout space
/// (used for overlays such as "Copy All" and the card's gear icon).
pub(crate) fn overlay_button(
    ui: &Ui,
    rect: Rect,
    id: egui::Id,
    label: &str,
    font: FontId,
    style: ButtonStyle,
) -> Response {
    let response = ui
        .interact(rect, id, Sense::CLICK)
        .on_hover_cursor(CursorIcon::PointingHand);
    paint_button(ui, rect, &response, label, font, style);
    response
}

/// Width a text button needs for `label` with horizontal padding.
pub(crate) fn text_button_width(ui: &Ui, label: &str, font: FontId, pad_x: f32) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    galley.size().x + 2.0 * pad_x
}

/// A full-width rounded box that centers its content.
pub(crate) fn boxed<R>(
    ui: &mut Ui,
    fill: Color32,
    stroke: Stroke,
    radius: u8,
    margin: Margin,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(radius)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(add_contents).inner
        })
}

/// Vector gear icon (circle body + 8 teeth + center hole) — no emoji font dependency.
pub(crate) fn paint_gear(painter: &Painter, center: Pos2, radius: f32, color: Color32, hole: Color32) {
    const TEETH: usize = 8;
    let r_body = radius * 0.72;
    let r_root = radius * 0.62;
    let half_w_outer = radius * 0.17;
    let half_w_inner = radius * 0.23;
    for i in 0..TEETH {
        let angle = i as f32 * TAU / TEETH as f32;
        let dir = Vec2::angled(angle);
        let perp = Vec2::angled(angle + FRAC_PI_2);
        let points = vec![
            center + dir * r_root - perp * half_w_inner,
            center + dir * radius - perp * half_w_outer,
            center + dir * radius + perp * half_w_outer,
            center + dir * r_root + perp * half_w_inner,
        ];
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
    painter.circle_filled(center, r_body, color);
    painter.circle_filled(center, radius * 0.30, hole);
}

/// Vector "✕" close glyph.
pub(crate) fn paint_close_x(painter: &Painter, center: Pos2, half: f32, color: Color32) {
    let stroke = Stroke::new(1.6_f32, color);
    painter.line_segment(
        [center + Vec2::new(-half, -half), center + Vec2::new(half, half)],
        stroke,
    );
    painter.line_segment(
        [center + Vec2::new(-half, half), center + Vec2::new(half, -half)],
        stroke,
    );
}

/// Small round icon button with a vector "✕" (dismiss / close).
pub(crate) fn close_icon_button(ui: &mut Ui, size: f32, color: Color32, hover_bg: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::CLICK);
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter()
                .circle_filled(rect.center(), size * 0.5, hover_bg);
        }
        paint_close_x(ui.painter(), rect.center(), size * 0.2, color);
    }
    response
}

/// Paints a horizontal dashed rule across `[x0, x1]` at `y`.
pub(crate) fn paint_dashed_rule(painter: &Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    let shapes = Shape::dashed_line(
        &[Pos2::new(x0, y), Pos2::new(x1, y)],
        Stroke::new(1.0_f32, color),
        4.0,
        3.0,
    );
    painter.extend(shapes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_cycle_is_visible_first_half_hidden_second_half() {
        let (v0, t0) = blink_phase(Duration::from_millis(0));
        assert!(v0);
        assert_eq!(t0, Duration::from_millis(600));

        let (v1, t1) = blink_phase(Duration::from_millis(599));
        assert!(v1);
        assert_eq!(t1, Duration::from_millis(1));

        let (v2, t2) = blink_phase(Duration::from_millis(600));
        assert!(!v2);
        assert_eq!(t2, Duration::from_millis(600));

        let (v3, _) = blink_phase(Duration::from_millis(1199));
        assert!(!v3);

        let (v4, t4) = blink_phase(Duration::from_millis(1250));
        assert!(v4);
        assert_eq!(t4, Duration::from_millis(550));
    }

    #[test]
    fn pixel_snapping_lands_on_physical_grid() {
        assert_eq!(snap_to_pixel(10.3, 1.0), 10.0);
        assert_eq!(snap_to_pixel(10.3, 2.0), 10.5);
        assert_eq!(snap_to_pixel(10.2, 1.5), 10.0);
        assert!((snap_to_pixel(7.77, 1.25) * 1.25 - (7.77f32 * 1.25).round()).abs() < 1e-4);
        assert_eq!(snap_to_pixel(3.3, 0.0), 3.3);
    }

    #[test]
    fn card_is_404_wide_in_default_window_and_shrinks_gracefully() {
        assert_eq!(card_width(460.0), CARD_MAX_WIDTH);
        assert_eq!(card_width(1200.0), CARD_MAX_WIDTH);
        assert_eq!(card_width(400.0), 400.0 - 2.0 * CARD_MIN_SIDE_MARGIN);
        assert_eq!(card_width(10.0), 0.0);
        // The 300 px QR frame always fits inside the card at the minimum window width.
        assert!(card_width(400.0) - 2.0 * 24.0 >= QR_PREVIEW_SIZE);
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
    }

    #[test]
    fn font_sniffing_rejects_collections_and_garbage() {
        let mut ttf = vec![0x00, 0x01, 0x00, 0x00];
        ttf.resize(4096, 0);
        assert!(looks_like_single_font(&ttf));
        let mut ttc = b"ttcf".to_vec();
        ttc.resize(4096, 0);
        assert!(!looks_like_single_font(&ttc));
        assert!(!looks_like_single_font(b"OTTO"));
    }
}
