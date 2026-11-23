//! QR code matrix generation (Error Correction Level H) and animation module.

pub mod animation;

use qrcode::{Color, EcLevel, QrCode};

/// Minimum quiet zone around the QR matrix in module units (ISO/IEC 18004 standard).
pub const QUIET_ZONE_MODULES: usize = 4;

/// Square boolean QR matrix encoded at Error Correction Level H.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    pub width: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// Encodes `payload` into a QR matrix with Error Correction Level H (30% recovery).
    pub fn encode(payload: &str) -> Result<Self, String> {
        encode(payload)
    }

    /// Returns `true` if the module at `(row, col)` is dark (`#000000`).
    /// Returns `false` if out of bounds.
    #[inline]
    pub fn is_dark(&self, row: usize, col: usize) -> bool {
        if row >= self.width || col >= self.width {
            return false;
        }
        self.dark[row * self.width + col]
    }

    /// Iterates over `(row, col)` coordinates of every dark module in row-major order.
    pub fn dark_modules(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let width = self.width;
        self.dark.iter().enumerate().filter_map(move |(idx, &is_dark)| {
            if is_dark {
                Some((idx / width, idx % width))
            } else {
                None
            }
        })
    }

    /// Returns the total number of dark modules in the matrix.
    pub fn dark_count(&self) -> usize {
        self.dark.iter().filter(|&&d| d).count()
    }
}

/// Encodes `payload` into a [`QrMatrix`] using Error Correction Level `EcLevel::H`.
pub fn encode(payload: &str) -> Result<QrMatrix, String> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::H)
        .map_err(|err| format!("Failed to encode QR code: {err}"))?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| matches!(c, Color::Dark))
        .collect::<Vec<_>>();
    Ok(QrMatrix { width, dark })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_amazon_payload_with_valid_finder_patterns() {
        let matrix = encode("AFNLFBF|RMA123456789|EXTRA").expect("QR encode should succeed");
        assert!(matrix.width >= 21);
        assert_eq!(matrix.dark.len(), matrix.width * matrix.width);
        assert!(matrix.dark_count() > 0);

        // Top-left 7x7 finder corner (0,0) and center (3,3) are always dark
        assert!(matrix.is_dark(0, 0));
        assert!(matrix.is_dark(3, 3));
        assert!(!matrix.is_dark(1, 1));

        // Out-of-bounds is safely false
        assert!(!matrix.is_dark(matrix.width, 0));
        assert!(!matrix.is_dark(0, matrix.width));

        assert_eq!(matrix.dark_modules().count(), matrix.dark_count());
    }
}
