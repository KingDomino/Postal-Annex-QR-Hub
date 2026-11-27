//! UTF-16 helpers for Win32 APIs (`#[cfg(windows)]`).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// Encodes a string or path as a null-terminated UTF-16 buffer for `PCWSTR` arguments.
pub(crate) fn to_wide_null(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}
