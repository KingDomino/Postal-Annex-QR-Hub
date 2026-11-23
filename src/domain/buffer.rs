//! Invisible keyboard/HID scan capture buffer with 1-second inactivity auto-clear.

use std::time::{Duration, Instant};

/// Default inactivity timeout after which stray keystrokes are automatically cleared.
pub const DEFAULT_INACTIVITY_TIMEOUT: Duration = Duration::from_millis(1000);

/// Deterministic invisible scan buffer.
///
/// Accepts `Instant` explicitly on every mutating/query operation so all timing
/// behaviors can be verified deterministically in unit tests without sleeping.
#[derive(Debug, Clone)]
pub struct ScanBuffer {
    buffer: String,
    last_input_at: Option<Instant>,
    timeout: Duration,
}

impl Default for ScanBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanBuffer {
    /// Creates a new buffer with the default 1000 ms inactivity timeout.
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_INACTIVITY_TIMEOUT)
    }

    /// Creates a new buffer with a custom inactivity timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            buffer: String::new(),
            last_input_at: None,
            timeout,
        }
    }

    /// Returns the configured inactivity timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Checks whether the buffer has expired as of `now`.
    ///
    /// If non-empty and `now - last_input_at >= timeout`, clears the buffer and
    /// returns `true`. Otherwise returns `false`.
    pub fn tick(&mut self, now: Instant) -> bool {
        if !self.buffer.is_empty() {
            if let Some(last) = self.last_input_at {
                if now.saturating_duration_since(last) >= self.timeout {
                    self.clear();
                    return true;
                }
            }
        }
        false
    }

    /// Appends captured text (from `Event::Text` or `Event::Paste`) into the buffer.
    ///
    /// First invokes [`Self::tick`] so expired stray characters can never corrupt a new scan.
    /// Strips `\r` and `\n` while preserving internal spaces, punctuation, and casing.
    pub fn push_str(&mut self, s: &str, now: Instant) {
        self.tick(now);

        let initial_len = self.buffer.len();
        for ch in s.chars() {
            if ch != '\r' && ch != '\n' {
                self.buffer.push(ch);
            }
        }

        if self.buffer.len() > initial_len {
            self.last_input_at = Some(now);
        }
    }

    /// Submits the current buffer (triggered on `Enter`).
    ///
    /// First invokes [`Self::tick`] so expired stray input is discarded rather than submitted.
    /// Clears the buffer and timestamp immediately. Returns `None` if the buffer is empty
    /// or whitespace-only; otherwise returns `Some(raw)` preserving internal whitespace.
    pub fn submit(&mut self, now: Instant) -> Option<String> {
        self.tick(now);

        let taken = std::mem::take(&mut self.buffer);
        self.last_input_at = None;

        if taken.trim().is_empty() {
            None
        } else {
            Some(taken)
        }
    }

    /// Clears the buffer and resets the inactivity timer.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.last_input_at = None;
    }

    /// Returns `true` if no characters are currently buffered.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Returns the byte length of the buffered string.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the current buffered string slice.
    pub fn as_str(&self) -> &str {
        &self.buffer
    }

    /// Returns the remaining duration until the buffer expires, or `None` if empty.
    pub fn remaining_timeout(&self, now: Instant) -> Option<Duration> {
        if self.buffer.is_empty() {
            return None;
        }
        let last = self.last_input_at?;
        let elapsed = now.saturating_duration_since(last);
        Some(self.timeout.saturating_sub(elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_burst_submits_and_clears_buffer() {
        let mut buf = ScanBuffer::new();
        let t0 = Instant::now();
        let payload = "AFNLFBF|RMA12345|EXTRA";

        for (i, ch) in payload.chars().enumerate() {
            let t = t0 + Duration::from_millis((i as u64) * 10);
            let mut s = [0u8; 4];
            buf.push_str(ch.encode_utf8(&mut s), t);
        }

        let submit_t = t0 + Duration::from_millis((payload.len() as u64) * 10 + 15);
        assert_eq!(buf.submit(submit_t), Some(payload.to_string()));
        assert!(buf.is_empty());
        assert_eq!(buf.remaining_timeout(submit_t), None);
    }

    #[test]
    fn stray_character_auto_clears_after_1000ms_without_corrupting_next_scan() {
        let mut buf = ScanBuffer::new();
        let t0 = Instant::now();

        // Associate accidentally taps 'x'
        buf.push_str("x", t0);
        assert_eq!(buf.as_str(), "x");

        // At 999 ms it has not expired yet
        assert!(!buf.tick(t0 + Duration::from_millis(999)));
        assert_eq!(buf.as_str(), "x");

        // At 1000 ms it auto-clears
        assert!(buf.tick(t0 + Duration::from_millis(1000)));
        assert!(buf.is_empty());

        // Subsequent real scan arrives at 1500 ms uncorrupted
        let t_scan = t0 + Duration::from_millis(1500);
        buf.push_str("AFNLFBF|CLEAN123", t_scan);
        assert_eq!(
            buf.submit(t_scan + Duration::from_millis(20)),
            Some("AFNLFBF|CLEAN123".to_string())
        );
    }

    #[test]
    fn expired_stray_char_cleared_automatically_on_next_push_even_without_explicit_tick() {
        let mut buf = ScanBuffer::new();
        let t0 = Instant::now();

        buf.push_str("z", t0);
        // No explicit tick; real scan starts 1200ms later
        let t1 = t0 + Duration::from_millis(1200);
        buf.push_str("HR|99999", t1);
        assert_eq!(
            buf.submit(t1 + Duration::from_millis(5)),
            Some("HR|99999".to_string())
        );
    }

    #[test]
    fn characters_spaced_900ms_apart_stay_alive_then_clear_after_1000ms_pause() {
        let mut buf = ScanBuffer::new();
        let t0 = Instant::now();

        buf.push_str("A", t0);
        assert_eq!(
            buf.remaining_timeout(t0 + Duration::from_millis(400)),
            Some(Duration::from_millis(600))
        );

        let t1 = t0 + Duration::from_millis(900);
        assert!(!buf.tick(t1));
        buf.push_str("B", t1);
        assert_eq!(buf.as_str(), "AB");

        let t2 = t1 + Duration::from_millis(900);
        assert!(!buf.tick(t2));
        buf.push_str("C", t2);
        assert_eq!(buf.as_str(), "ABC");

        // Pausing 1000 ms after the last character clears it
        let t3 = t2 + Duration::from_millis(1000);
        assert!(buf.tick(t3));
        assert!(buf.is_empty());
        assert_eq!(buf.submit(t3), None);
    }

    #[test]
    fn empty_and_whitespace_only_enter_are_ignored_while_internal_spaces_preserved() {
        let mut buf = ScanBuffer::new();
        let t0 = Instant::now();

        // Empty submit
        assert_eq!(buf.submit(t0), None);

        // Whitespace-only submit
        buf.push_str("   \t  ", t0);
        assert_eq!(buf.submit(t0 + Duration::from_millis(10)), None);
        assert!(buf.is_empty());

        // Preserves internal whitespace and strips CR/LF
        buf.push_str("AFNLFBF|RMA 123|DATA\r\n", t0 + Duration::from_millis(20));
        assert_eq!(
            buf.submit(t0 + Duration::from_millis(30)),
            Some("AFNLFBF|RMA 123|DATA".to_string())
        );
    }
}
