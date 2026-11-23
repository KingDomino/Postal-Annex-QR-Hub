//! Per-module staggered block-assembly animation math for the on-screen QR preview.

use std::time::Instant;

/// Window over which modules stagger their start times (seconds).
pub const STAGGER_WINDOW_SECS: f32 = 0.34;

/// Duration of each individual module's materialization animation (seconds).
pub const MODULE_DURATION_SECS: f32 = 0.18;

/// Total duration until every dark module has snapped cleanly into place (seconds).
pub const TOTAL_DURATION_SECS: f32 = STAGGER_WINDOW_SECS + MODULE_DURATION_SECS;

/// Per-module rendered transform and opacity at a given point in the animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleAnimState {
    /// Scale factor in `[0.0, 1.0]` around the module's cell center.
    pub scale: f32,
    /// Horizontal micro-assembly displacement in logical pixels (approaches `0.0`).
    pub offset_x_px: f32,
    /// Vertical micro-assembly displacement in logical pixels (approaches `0.0`).
    pub offset_y_px: f32,
    /// Opacity in `0..=255`.
    pub alpha: u8,
}

/// Tracks the start time and scan nonce for an active QR materialization animation.
#[derive(Debug, Clone, Copy)]
pub struct QrAnimation {
    pub scan_nonce: u64,
    pub started_at: Instant,
}

impl QrAnimation {
    /// Creates a new animation state for `scan_nonce` starting at `started_at`.
    pub fn new(scan_nonce: u64, started_at: Instant) -> Self {
        Self {
            scan_nonce,
            started_at,
        }
    }

    /// Elapsed time in seconds since [`Self::started_at`].
    pub fn elapsed_secs(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.started_at).as_secs_f32()
    }

    /// Returns `true` once all modules have completed animation at `now`.
    pub fn is_finished_at(&self, now: Instant) -> bool {
        Self::is_finished_elapsed(self.elapsed_secs(now))
    }

    /// Returns `true` if `elapsed_secs >= TOTAL_DURATION_SECS`.
    pub fn is_finished_elapsed(elapsed_secs: f32) -> bool {
        elapsed_secs >= TOTAL_DURATION_SECS
    }
}

/// Deterministic SplitMix64 mixing function.
#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Converts a `u64` hash into a deterministic `f32` in `[0.0, 1.0)`.
#[inline]
fn hash_to_unit_f32(hash: u64) -> f32 {
    // Use top 24 bits to map uniformly into [0.0, 1.0).
    let bits = (hash >> 40) as u32;
    (bits as f32) / ((1u32 << 24) as f32)
}

/// Computes the exact animation state of module `(row, col)` at `elapsed_secs`.
///
/// Implements the `SPEC.md` §6.2 formula:
/// - `t_start = (0.65 * h + 0.35 * d) * 0.34`
/// - `u = clamp((elapsed_secs - t_start) / 0.18, 0.0, 1.0)`
/// - `e = 1.0 - (1.0 - u)^3`
/// - `scale = e`, `offset = drift * (1.0 - e)`, `alpha = round(e * 255.0)`
pub fn module_state(
    row: usize,
    col: usize,
    matrix_width: usize,
    scan_nonce: u64,
    elapsed_secs: f32,
) -> ModuleAnimState {
    if elapsed_secs >= TOTAL_DURATION_SECS {
        return ModuleAnimState {
            scale: 1.0,
            offset_x_px: 0.0,
            offset_y_px: 0.0,
            alpha: 255,
        };
    }

    // Mix (row, col, scan_nonce) with SplitMix64 for uncorrelated per-scan randomness.
    let seed = (scan_nonce.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        ^ ((row as u64).wrapping_mul(0x517c_c1b7_2722_0a95))
        ^ ((col as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
    let h1 = splitmix64(seed);
    let h2 = splitmix64(h1);
    let h3 = splitmix64(h2);

    let h = hash_to_unit_f32(h1);

    let half = (matrix_width.saturating_sub(1) as f32) * 0.5;
    let dx = col as f32 - half;
    let dy = row as f32 - half;
    let dist = (dx * dx + dy * dy).sqrt();
    let max_dist = (half * half * 2.0).sqrt().max(1.0);
    let d = (dist / max_dist).clamp(0.0, 1.0);

    let t_start = (0.65 * h + 0.35 * d) * STAGGER_WINDOW_SECS;
    let u = if elapsed_secs <= 0.0 {
        0.0
    } else {
        ((elapsed_secs - t_start) / MODULE_DURATION_SECS).clamp(0.0, 1.0)
    };
    let e = 1.0 - (1.0 - u).powi(3);

    // Deterministic outward + micro-angle drift with magnitude in [3.0, 6.0] px.
    let drift_mag = 3.0 + 3.0 * hash_to_unit_f32(h2);
    let base_angle = if dist > 1e-4 {
        dy.atan2(dx)
    } else {
        hash_to_unit_f32(h3) * std::f32::consts::TAU
    };
    let angle_jitter = (hash_to_unit_f32(h3) - 0.5) * 0.7;
    let angle = base_angle + angle_jitter;

    let remaining = 1.0 - e;
    let offset_x_px = angle.cos() * drift_mag * remaining;
    let offset_y_px = angle.sin() * drift_mag * remaining;
    let alpha = (e * 255.0).round().clamp(0.0, 255.0) as u8;

    ModuleAnimState {
        scale: e,
        offset_x_px,
        offset_y_px,
        alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn animation_transitions_monotonically_from_hidden_to_snapped_final_state() {
        let width = 29;
        let nonce = 7;
        let sample_times = [0.0, 0.05, 0.12, 0.22, 0.34, 0.45, TOTAL_DURATION_SECS, 0.9];

        for row in (0..width).step_by(3) {
            for col in (0..width).step_by(3) {
                let initial = module_state(row, col, width, nonce, 0.0);
                assert_eq!(initial.scale, 0.0);
                assert_eq!(initial.alpha, 0);

                let mut prev_scale = initial.scale;
                let mut prev_alpha = initial.alpha;
                let mut prev_offset_mag =
                    (initial.offset_x_px.powi(2) + initial.offset_y_px.powi(2)).sqrt();
                assert!((2.99..=6.01).contains(&prev_offset_mag));

                for &t in &sample_times[1..] {
                    let st = module_state(row, col, width, nonce, t);
                    assert!(
                        st.scale >= prev_scale - 1e-6,
                        "scale decreased at ({row},{col}) t={t}: {} < {prev_scale}",
                        st.scale
                    );
                    assert!(st.alpha >= prev_alpha);

                    let mag = (st.offset_x_px.powi(2) + st.offset_y_px.powi(2)).sqrt();
                    assert!(
                        mag <= prev_offset_mag + 1e-5,
                        "offset grew at ({row},{col}) t={t}: {mag} > {prev_offset_mag}"
                    );

                    prev_scale = st.scale;
                    prev_alpha = st.alpha;
                    prev_offset_mag = mag;
                }

                let final_state = module_state(row, col, width, nonce, TOTAL_DURATION_SECS);
                assert_eq!(final_state.scale, 1.0);
                assert_eq!(final_state.offset_x_px, 0.0);
                assert_eq!(final_state.offset_y_px, 0.0);
                assert_eq!(final_state.alpha, 255);
            }
        }
    }

    #[test]
    fn qr_animation_struct_tracks_elapsed_and_completion() {
        let t0 = Instant::now();
        let anim = QrAnimation::new(1, t0);
        assert!(!anim.is_finished_at(t0));
        assert!(!anim.is_finished_at(t0 + Duration::from_millis(400)));
        assert!(anim.is_finished_at(t0 + Duration::from_millis(520)));
        assert!(anim.is_finished_at(t0 + Duration::from_millis(800)));
    }
}
