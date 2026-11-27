//! Seamless borderless window chrome: minimalist titlebar, drag/maximize, pixel-snapped
//! vector window controls, 8-way edge/corner resizing, and a subtle outer window border.

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Frame, Id, LayerId, Layout, Margin, Order,
    Painter, PointerButton, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Vec2,
    ViewportCommand,
};

use super::colors::*;
use super::{font_semibold, label, snap_to_pixel};

/// Height of the custom borderless titlebar in logical pixels.
pub(super) const TITLEBAR_HEIGHT: f32 = 34.0;

/// Interactive hit zone width along window edges and corners for resizing.
const RESIZE_BORDER_PX: f32 = 5.0;

/// Width of each right-side window control button (minimize, maximize, close).
const CONTROL_BTN_WIDTH: f32 = 40.0;

/// Subtle 1 px outer window border so the seamless frame reads cleanly on any desktop.
const WINDOW_BORDER_COLOR: Color32 = Color32::from_rgb(0xcf, 0xd6, 0xdf);

/// Renders the borderless window chrome, 8-way resize hit-testing, and outer hairline border.
pub(super) fn show(ctx: &egui::Context) {
    let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let screen_rect = ctx.screen_rect();

    let active_resize_edge = if !is_maximized {
        handle_edge_resize(ctx, screen_rect)
    } else {
        None
    };

    egui::TopBottomPanel::top("window_chrome")
        .exact_height(TITLEBAR_HEIGHT)
        .frame(Frame::new().fill(WINDOW_BG).inner_margin(Margin::ZERO))
        .show_separator_line(false)
        .resizable(false)
        .show(ctx, |ui| {
            let bar_rect = ui.max_rect();
            let controls_width = CONTROL_BTN_WIDTH * 3.0;
            let drag_rect = Rect::from_min_max(
                bar_rect.min,
                Pos2::new(
                    (bar_rect.max.x - controls_width).max(bar_rect.min.x),
                    bar_rect.max.y,
                ),
            );

            // Non-focusable drag + double-click maximize region (excludes right control buttons).
            let drag_response = ui.interact(
                drag_rect,
                ui.id().with("chrome_titlebar_drag"),
                Sense::CLICK | Sense::DRAG,
            );
            if active_resize_edge.is_none() && drag_response.drag_started_by(PointerButton::Primary)
            {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag_response.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_maximized));
            }

            // Left: compact vector QR Hub mark + title.
            let brand_rect = drag_rect.shrink2(Vec2::new(10.0, 0.0));
            ui.allocate_new_ui(
                egui::UiBuilder::new()
                    .max_rect(brand_rect)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    let (mark_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    if ui.is_rect_visible(mark_rect) {
                        paint_mini_qr_mark(ui.painter(), mark_rect);
                    }
                    ui.add(label("QR Hub", font_semibold(12.5), NAVY));
                },
            );

            // Right: minimize, maximize/restore, and close buttons (Sense::CLICK only).
            let controls_rect = Rect::from_min_max(
                Pos2::new(
                    (bar_rect.max.x - controls_width).max(bar_rect.min.x),
                    bar_rect.min.y,
                ),
                bar_rect.max,
            );
            ui.allocate_new_ui(
                egui::UiBuilder::new()
                    .max_rect(controls_rect)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let ppp = ctx.pixels_per_point();

                    if window_control_button(ui, "chrome_close", "Close", true, |p, r, fg, _| {
                        paint_close_glyph(p, r.center(), ppp, fg);
                    })
                    .clicked()
                    {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }

                    let max_tooltip = if is_maximized { "Restore" } else { "Maximize" };
                    if window_control_button(
                        ui,
                        "chrome_maximize",
                        max_tooltip,
                        false,
                        |p, r, fg, bg| {
                            if is_maximized {
                                paint_restore_glyph(p, r.center(), ppp, fg, bg);
                            } else {
                                paint_maximize_glyph(p, r.center(), ppp, fg);
                            }
                        },
                    )
                    .clicked()
                    {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_maximized));
                    }

                    if window_control_button(
                        ui,
                        "chrome_minimize",
                        "Minimize",
                        false,
                        |p, r, fg, _| {
                            paint_minimize_glyph(p, r.center(), ppp, fg);
                        },
                    )
                    .clicked()
                    {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }
                },
            );
        });

    if !is_maximized {
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("window_border")));
        painter.rect_stroke(
            screen_rect,
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, WINDOW_BORDER_COLOR),
            StrokeKind::Inside,
        );
    }
}

/// Detects pointer hover/press along the outer resize border, updates the cursor, and
/// dispatches [`ViewportCommand::BeginResize`] on primary press.
fn handle_edge_resize(ctx: &egui::Context, screen_rect: Rect) -> Option<ResizeDirection> {
    let (hover_pos, primary_pressed) =
        ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed()));
    let dir = hover_pos.and_then(|pos| resize_direction_at(screen_rect, pos, RESIZE_BORDER_PX))?;
    ctx.set_cursor_icon(resize_cursor(dir));
    if primary_pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
    Some(dir)
}

/// Returns the 8-way [`ResizeDirection`] when `pos` lies within `border` logical pixels of
/// `screen_rect`'s edges or corners, or `None` when inside the interior or outside the window.
pub(crate) fn resize_direction_at(
    screen_rect: Rect,
    pos: Pos2,
    border: f32,
) -> Option<ResizeDirection> {
    if border <= 0.0 || !screen_rect.contains(pos) {
        return None;
    }

    let left = pos.x - screen_rect.left() <= border;
    let right = screen_rect.right() - pos.x <= border;
    let top = pos.y - screen_rect.top() <= border;
    let bottom = screen_rect.bottom() - pos.y <= border;

    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        _ => None,
    }
}

/// Maps an 8-way [`ResizeDirection`] to its matching [`CursorIcon`].
pub(crate) fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
    }
}

fn window_control_button(
    ui: &mut egui::Ui,
    id_salt: &str,
    tooltip: &str,
    is_close: bool,
    paint_glyph: impl FnOnce(&Painter, Rect, Color32, Color32),
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(CONTROL_BTN_WIDTH, TITLEBAR_HEIGHT),
        Sense::hover(),
    );
    let response = ui
        .interact(rect, ui.id().with(id_salt), Sense::CLICK)
        .on_hover_text(tooltip);

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let down = response.is_pointer_button_down_on();
        let (bg, fg) = if is_close {
            if down {
                (RED_HOVER, WHITE)
            } else if hovered {
                (RED, WHITE)
            } else {
                (WINDOW_BG, TEXT_BODY)
            }
        } else if down {
            (GRAY_300, TEXT_DARK)
        } else if hovered {
            (GRAY_200, TEXT_DARK)
        } else {
            (WINDOW_BG, TEXT_BODY)
        };

        let painter = ui.painter();
        if bg != WINDOW_BG {
            painter.rect_filled(rect, CornerRadius::ZERO, bg);
        }
        paint_glyph(painter, rect, fg, bg);
    }

    response
}

/// Compact 14x14 vector QR Hub mark: navy rounded square, three white mini finder squares,
/// and a tiny brand-red `+` in the bottom-right corner.
fn paint_mini_qr_mark(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::same(3), NAVY);

    let finder = 3.2;
    let pad = 2.4;
    let finder_positions = [
        Pos2::new(rect.left() + pad, rect.top() + pad),
        Pos2::new(rect.right() - pad - finder, rect.top() + pad),
        Pos2::new(rect.left() + pad, rect.bottom() - pad - finder),
    ];
    for min in finder_positions {
        painter.rect_filled(Rect::from_min_size(min, Vec2::splat(finder)), 0.5, WHITE);
    }

    // Tiny brand-red '+' in the bottom-right quadrant.
    let plus_center = Pos2::new(
        rect.right() - pad - finder * 0.5,
        rect.bottom() - pad - finder * 0.5,
    );
    let arm = 1.9;
    let stroke = Stroke::new(1.3_f32, BRAND_RED);
    painter.line_segment(
        [
            Pos2::new(plus_center.x - arm, plus_center.y),
            Pos2::new(plus_center.x + arm, plus_center.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(plus_center.x, plus_center.y - arm),
            Pos2::new(plus_center.x, plus_center.y + arm),
        ],
        stroke,
    );
}

fn paint_minimize_glyph(painter: &Painter, center: Pos2, ppp: f32, color: Color32) {
    let y = snap_to_pixel(center.y + 0.5, ppp);
    let x0 = snap_to_pixel(center.x - 5.0, ppp);
    let x1 = snap_to_pixel(center.x + 5.0, ppp);
    painter.line_segment(
        [Pos2::new(x0, y), Pos2::new(x1, y)],
        Stroke::new(1.25_f32, color),
    );
}

fn paint_maximize_glyph(painter: &Painter, center: Pos2, ppp: f32, color: Color32) {
    let min = Pos2::new(
        snap_to_pixel(center.x - 4.5, ppp),
        snap_to_pixel(center.y - 4.5, ppp),
    );
    let max = Pos2::new(
        snap_to_pixel(center.x + 4.5, ppp),
        snap_to_pixel(center.y + 4.5, ppp),
    );
    painter.rect_stroke(
        Rect::from_min_max(min, max),
        CornerRadius::ZERO,
        Stroke::new(1.2_f32, color),
        StrokeKind::Inside,
    );
}

fn paint_restore_glyph(painter: &Painter, center: Pos2, ppp: f32, color: Color32, bg: Color32) {
    let stroke = Stroke::new(1.15_f32, color);
    // Back square shifted up-right.
    let back = Rect::from_min_max(
        Pos2::new(
            snap_to_pixel(center.x - 2.5, ppp),
            snap_to_pixel(center.y - 4.5, ppp),
        ),
        Pos2::new(
            snap_to_pixel(center.x + 4.5, ppp),
            snap_to_pixel(center.y + 2.5, ppp),
        ),
    );
    painter.rect_stroke(back, CornerRadius::ZERO, stroke, StrokeKind::Inside);

    // Front square shifted down-left, filled with button background to mask overlapping edges.
    let front = Rect::from_min_max(
        Pos2::new(
            snap_to_pixel(center.x - 4.5, ppp),
            snap_to_pixel(center.y - 2.5, ppp),
        ),
        Pos2::new(
            snap_to_pixel(center.x + 2.5, ppp),
            snap_to_pixel(center.y + 4.5, ppp),
        ),
    );
    painter.rect(front, CornerRadius::ZERO, bg, stroke, StrokeKind::Inside);
}

fn paint_close_glyph(painter: &Painter, center: Pos2, ppp: f32, color: Color32) {
    let cx = snap_to_pixel(center.x, ppp);
    let cy = snap_to_pixel(center.y, ppp);
    let half = 4.5;
    let stroke = Stroke::new(1.3_f32, color);
    painter.line_segment(
        [
            Pos2::new(cx - half, cy - half),
            Pos2::new(cx + half, cy + half),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(cx - half, cy + half),
            Pos2::new(cx + half, cy - half),
        ],
        stroke,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_direction_detects_all_eight_edges_and_corners() {
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(460.0, 740.0));
        let b = RESIZE_BORDER_PX;

        assert_eq!(
            resize_direction_at(rect, Pos2::new(2.0, 2.0), b),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(458.0, 2.0), b),
            Some(ResizeDirection::NorthEast)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(2.0, 738.0), b),
            Some(ResizeDirection::SouthWest)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(458.0, 738.0), b),
            Some(ResizeDirection::SouthEast)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(230.0, 1.0), b),
            Some(ResizeDirection::North)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(230.0, 739.0), b),
            Some(ResizeDirection::South)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(3.0, 350.0), b),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            resize_direction_at(rect, Pos2::new(457.0, 350.0), b),
            Some(ResizeDirection::East)
        );
        // Interior and outside points do not trigger resize.
        assert_eq!(resize_direction_at(rect, Pos2::new(230.0, 350.0), b), None);
        assert_eq!(resize_direction_at(rect, Pos2::new(-1.0, 10.0), b), None);
        assert_eq!(resize_direction_at(rect, Pos2::new(500.0, 10.0), b), None);
    }

    #[test]
    fn resize_cursor_maps_every_direction() {
        assert_eq!(
            resize_cursor(ResizeDirection::North),
            CursorIcon::ResizeNorth
        );
        assert_eq!(
            resize_cursor(ResizeDirection::South),
            CursorIcon::ResizeSouth
        );
        assert_eq!(resize_cursor(ResizeDirection::East), CursorIcon::ResizeEast);
        assert_eq!(resize_cursor(ResizeDirection::West), CursorIcon::ResizeWest);
        assert_eq!(
            resize_cursor(ResizeDirection::NorthWest),
            CursorIcon::ResizeNorthWest
        );
        assert_eq!(
            resize_cursor(ResizeDirection::NorthEast),
            CursorIcon::ResizeNorthEast
        );
        assert_eq!(
            resize_cursor(ResizeDirection::SouthWest),
            CursorIcon::ResizeSouthWest
        );
        assert_eq!(
            resize_cursor(ResizeDirection::SouthEast),
            CursorIcon::ResizeSouthEast
        );
    }
}
