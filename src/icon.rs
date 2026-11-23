//! Deterministic pure-Rust PostalAnnex+ window icon renderer.
//!
//! Renders a 64x64 RGBA icon matching `assets/icon.ico`:
//! - PostalAnnex navy (`#002f6c`) rounded square background
//! - Crisp white QR finder pattern & accent modules
//! - PostalAnnex red (`#d11226`) circular `+` badge in the bottom-right corner

/// Generates a deterministic 64x64 RGBA buffer suitable for `egui::IconData`.
pub fn window_icon_rgba() -> (Vec<u8>, u32, u32) {
    const SIZE: u32 = 64;
    let rgba = render_icon_rgba(SIZE);
    (rgba, SIZE, SIZE)
}

/// Renders the PostalAnnex+ QR icon at any square resolution `size x size`.
pub fn render_icon_rgba(size: u32) -> Vec<u8> {
    const NAVY: [f32; 3] = [0.0, 47.0, 108.0]; // #002f6c
    const RED: [f32; 3] = [209.0, 18.0, 38.0]; // #d11226
    const WHITE: [f32; 3] = [255.0, 255.0, 255.0];

    let mut out = vec![0u8; (size * size * 4) as usize];
    let size_f = size as f32;
    let aa = (1.25 / size_f).max(0.005);

    // 11x11 white QR pattern placed inside [0.14, 0.86]
    let qr_min = 0.14_f32;
    let qr_max = 0.86_f32;
    let qr_span = qr_max - qr_min;
    let grid_n = 11_i32;

    // Red '+' badge geometry
    let bx = 0.74_f32;
    let by = 0.74_f32;
    let badge_halo_r = 0.235_f32;
    let badge_r = 0.195_f32;
    let plus_half_len = 0.105_f32;
    let plus_half_thick = 0.036_f32;

    for y in 0..size {
        let v = (y as f32 + 0.5) / size_f;
        for x in 0..size {
            let u = (x as f32 + 0.5) / size_f;

            // Signed distance to rounded square in [0.03, 0.97], corner radius 0.20
            let sd_bg = rounded_box_sdf(u - 0.5, v - 0.5, 0.47, 0.47, 0.20);
            let bg_alpha = smooth_coverage(sd_bg, aa);

            if bg_alpha <= 0.0 {
                continue;
            }

            let mut rgb = NAVY;

            // QR pattern layer
            if u >= qr_min && u < qr_max && v >= qr_min && v < qr_max {
                let col = (((u - qr_min) / qr_span) * grid_n as f32).floor() as i32;
                let row = (((v - qr_min) / qr_span) * grid_n as f32).floor() as i32;
                if is_qr_white_cell(row, col) {
                    rgb = WHITE;
                }
            }

            // Navy halo around red '+' badge to separate it cleanly from QR modules
            let dist_badge = ((u - bx) * (u - bx) + (v - by) * (v - by)).sqrt();
            let halo_cov = smooth_coverage(dist_badge - badge_halo_r, aa);
            if halo_cov > 0.0 {
                rgb = blend_rgb(rgb, NAVY, halo_cov);
            }

            // Red circle badge
            let red_cov = smooth_coverage(dist_badge - badge_r, aa);
            if red_cov > 0.0 {
                rgb = blend_rgb(rgb, RED, red_cov);
            }

            // White '+' symbol inside the red badge
            let dx = (u - bx).abs();
            let dy = (v - by).abs();
            let sd_h = box_sdf(dx, dy, plus_half_len, plus_half_thick);
            let sd_v = box_sdf(dx, dy, plus_half_thick, plus_half_len);
            let plus_cov = smooth_coverage(sd_h.min(sd_v), aa);
            if plus_cov > 0.0 {
                rgb = blend_rgb(rgb, WHITE, plus_cov);
            }

            let idx = ((y * size + x) * 4) as usize;
            out[idx] = rgb[0].round().clamp(0.0, 255.0) as u8;
            out[idx + 1] = rgb[1].round().clamp(0.0, 255.0) as u8;
            out[idx + 2] = rgb[2].round().clamp(0.0, 255.0) as u8;
            out[idx + 3] = (bg_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    out
}

fn is_qr_white_cell(row: i32, col: i32) -> bool {
    // Classic 7x7 QR finder pattern at top-left (rows 0..7, cols 0..7)
    if (0..7).contains(&row) && (0..7).contains(&col) {
        let on_outer_border = row == 0 || row == 6 || col == 0 || col == 6;
        let in_center = (2..=4).contains(&row) && (2..=4).contains(&col);
        return on_outer_border || in_center;
    }

    // Top-right QR data modules
    matches!(
        (row, col),
        (0, 8)
            | (0, 9)
            | (0, 10)
            | (1, 10)
            | (2, 8)
            | (2, 10)
            | (3, 8)
            | (3, 9)
            | (4, 10)
            | (5, 8)
    )
    // Bottom-left QR data modules
    || matches!(
        (row, col),
        (8, 0)
            | (8, 1)
            | (8, 2)
            | (8, 4)
            | (9, 0)
            | (9, 3)
            | (10, 0)
            | (10, 1)
            | (10, 2)
            | (10, 4)
    )
}

#[inline]
fn rounded_box_sdf(px: f32, py: f32, bx: f32, by: f32, radius: f32) -> f32 {
    let qx = px.abs() - (bx - radius);
    let qy = py.abs() - (by - radius);
    let out_dist = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let in_dist = qx.max(qy).min(0.0);
    out_dist + in_dist - radius
}

#[inline]
fn box_sdf(ax: f32, ay: f32, hx: f32, hy: f32) -> f32 {
    let dx = ax - hx;
    let dy = ay - hy;
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0)
}

#[inline]
fn smooth_coverage(sd: f32, aa: f32) -> f32 {
    ((0.5 * aa - sd) / aa).clamp(0.0, 1.0)
}

#[inline]
fn blend_rgb(dst: [f32; 3], src: [f32; 3], alpha: f32) -> [f32; 3] {
    [
        dst[0] * (1.0 - alpha) + src[0] * alpha,
        dst[1] * (1.0 - alpha) + src[1] * alpha,
        dst[2] * (1.0 - alpha) + src[2] * alpha,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_has_expected_dimensions_and_non_empty_pixels() {
        let (rgba, w, h) = window_icon_rgba();
        assert_eq!(w, 64);
        assert_eq!(h, 64);
        assert_eq!(rgba.len(), (64 * 64 * 4) as usize);

        // Center pixel should be opaque
        let center_idx = ((32 * 64 + 32) * 4) as usize;
        assert_eq!(rgba[center_idx + 3], 255);

        // Outer corner pixel (0, 0) should be transparent due to rounded corners
        assert_eq!(rgba[3], 0);

        // Red badge center (approx pixel 47, 47) should contain white '+'
        let badge_idx = ((47 * 64 + 47) * 4) as usize;
        assert!(rgba[badge_idx] > 200 && rgba[badge_idx + 1] > 200 && rgba[badge_idx + 2] > 200);
    }
}
