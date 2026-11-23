//! Cross-platform clipboard writer with retry-on-lock backoff for Windows.

use std::thread;
use std::time::Duration;

/// Default number of attempts to acquire the system clipboard.
pub const DEFAULT_MAX_RETRIES: usize = 5;

/// Default backoff delay between failed clipboard attempts.
pub const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(20);

/// Retries an operation up to `max_retries` times (at least once), sleeping `backoff`
/// between failed attempts.
pub fn retry_with_backoff<F, E>(
    max_retries: usize,
    backoff: Duration,
    mut op: F,
) -> Result<(), E>
where
    F: FnMut() -> Result<(), E>,
{
    let attempts = max_retries.max(1);
    let mut last_err: Option<E> = None;

    for attempt in 0..attempts {
        match op() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 < attempts && !backoff.is_zero() {
                    thread::sleep(backoff);
                }
            }
        }
    }

    Err(last_err.expect("at least one attempt ran"))
}

/// Writes `text` to the OS clipboard with 5 retries and 20 ms backoff to handle
/// transient `OpenClipboard` locks from PostalMate or Windows Clipboard History.
pub fn copy_text(text: &str) -> Result<(), String> {
    retry_with_backoff(DEFAULT_MAX_RETRIES, DEFAULT_RETRY_BACKOFF, || {
        copy_text_once(text)
    })
}

#[cfg(windows)]
fn copy_text_once(text: &str) -> Result<(), String> {
    clipboard_win::set_clipboard_string(text)
        .map_err(|err| format!("Failed to write Windows clipboard: {err}"))
}

#[cfg(not(windows))]
fn copy_text_once(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("Failed to open clipboard: {err}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|err| format!("Failed to write clipboard: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_succeeds_after_transient_failures() {
        let mut attempts = 0usize;
        let res: Result<(), &'static str> =
            retry_with_backoff(5, Duration::ZERO, || {
                attempts += 1;
                if attempts < 3 {
                    Err("OpenClipboard locked")
                } else {
                    Ok(())
                }
            });
        assert_eq!(res, Ok(()));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn retry_exhausts_all_attempts_and_returns_last_error() {
        let mut attempts = 0usize;
        let res: Result<(), usize> = retry_with_backoff(5, Duration::ZERO, || {
            attempts += 1;
            Err(attempts)
        });
        assert_eq!(res, Err(5));
        assert_eq!(attempts, 5);
    }
}
