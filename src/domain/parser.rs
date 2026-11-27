//! Caps Lock case-inversion engine, pipe splitting, and return route classifier.

/// Classified route for a scanned QR payload after Caps Lock normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanRoute {
    /// Priority 1: normalized payload starts with `"HR"`.
    HappyReturn {
        payload: String,
        rma: Option<String>,
    },
    /// Priority 2: first pipe-delimited section starts with `"AFNLFBF"`.
    NoBoxNoLabel {
        payload: String,
        rma: Option<String>,
    },
    /// Priority 3: first pipe-delimited section starts with `"AFNLF"`.
    CustomerPacked {
        payload: String,
        rma: Option<String>,
    },
    /// Priority 4: normalized payload starts with `"UNPK-"`.
    Unpacked { payload: String, content: String },
    /// Priority 5: normalized payload starts with `"1Z"`.
    UpsTracking { payload: String },
    /// Fallback: unrecognized return payload.
    Unrecognized { payload: String },
}

/// Inverts the case of every alphabetic character individually.
/// Uppercase becomes lowercase, lowercase becomes uppercase; digits and symbols are unchanged.
pub fn flip_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_uppercase() {
            out.extend(ch.to_lowercase());
        } else if ch.is_lowercase() {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Applies the pattern-based Caps Lock correction rule for known return prefixes.
///
/// If the text starts with a case-insensitive known prefix (`afnlf`, `hr`, `unpk-`, or `1z`)
/// AND its first alphabetic character is lowercase (`a`, `h`, `u`, or `1z`), Caps Lock was
/// inverted on the scanner/workstation, so every character's case is individually inverted
/// via [`flip_case`].
pub fn normalize_caps_lock(text: &str) -> String {
    let is_inverted_afnlf = text.starts_with('a')
        && text
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("afnlf"));
    let is_inverted_hr =
        text.starts_with('h') && text.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("hr"));
    let is_inverted_unpk = text.starts_with('u')
        && text
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("unpk-"));
    let is_inverted_1z = text.starts_with("1z");

    if is_inverted_afnlf || is_inverted_hr || is_inverted_unpk || is_inverted_1z {
        flip_case(text)
    } else {
        text.to_string()
    }
}

/// Normalizes Caps Lock inversion and classifies the scanned payload using
/// the route priority matrix.
pub fn classify_scan(raw_text: &str) -> ScanRoute {
    let normalized = normalize_caps_lock(raw_text);
    let mut sections = normalized.split('|');
    let first_section = sections.next().unwrap_or("");
    let rma = sections
        .next()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    if normalized.starts_with("HR") {
        ScanRoute::HappyReturn {
            payload: normalized,
            rma,
        }
    } else if first_section.starts_with("AFNLFBF") {
        ScanRoute::NoBoxNoLabel {
            payload: normalized,
            rma,
        }
    } else if first_section.starts_with("AFNLF") {
        ScanRoute::CustomerPacked {
            payload: normalized,
            rma,
        }
    } else if let Some(after_prefix) = normalized.strip_prefix("UNPK-") {
        ScanRoute::Unpacked {
            content: after_prefix.to_owned(),
            payload: normalized,
        }
    } else if normalized.starts_with("1Z") {
        ScanRoute::UpsTracking {
            payload: normalized,
        }
    } else {
        ScanRoute::Unrecognized {
            payload: normalized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_case_inverts_letters_and_preserves_digits_and_symbols() {
        assert_eq!(flip_case("afnlfbf|rMa123|xYz"), "AFNLFBF|RmA123|XyZ");
        assert_eq!(flip_case("hr|AbC999-!@#"), "HR|aBc999-!@#");
        assert_eq!(flip_case("12345|_-."), "12345|_-.");
    }

    #[test]
    fn caps_lock_inversion_spec_cases() {
        // afnlfbf|rMa123|xYz -> AFNLFBF|RmA123|XyZ -> NoBoxNoLabel, RMA RmA123
        let route = classify_scan("afnlfbf|rMa123|xYz");
        assert_eq!(
            route,
            ScanRoute::NoBoxNoLabel {
                payload: "AFNLFBF|RmA123|XyZ".to_string(),
                rma: Some("RmA123".to_string()),
            }
        );

        // hr|AbC999 -> HR|aBc999 -> HappyReturn
        let route = classify_scan("hr|AbC999");
        assert_eq!(
            route,
            ScanRoute::HappyReturn {
                payload: "HR|aBc999".to_string(),
                rma: Some("aBc999".to_string()),
            }
        );

        // AFNLFBF|AbC -> unchanged
        assert_eq!(normalize_caps_lock("AFNLFBF|AbC"), "AFNLFBF|AbC");
        let route = classify_scan("AFNLFBF|AbC");
        assert_eq!(
            route,
            ScanRoute::NoBoxNoLabel {
                payload: "AFNLFBF|AbC".to_string(),
                rma: Some("AbC".to_string()),
            }
        );

        // unknown|test -> NOT flipped -> Unrecognized
        assert_eq!(normalize_caps_lock("unknown|test"), "unknown|test");
        let route = classify_scan("unknown|test");
        assert_eq!(
            route,
            ScanRoute::Unrecognized {
                payload: "unknown|test".to_string(),
            }
        );
    }

    #[test]
    fn routing_priority_and_missing_or_empty_rma() {
        // Priority 1: HR even without pipe
        assert_eq!(
            classify_scan("HR12345"),
            ScanRoute::HappyReturn {
                payload: "HR12345".to_string(),
                rma: None,
            }
        );

        // Priority 2: AFNLFBF beats AFNLF
        assert_eq!(
            classify_scan("AFNLFBF"),
            ScanRoute::NoBoxNoLabel {
                payload: "AFNLFBF".to_string(),
                rma: None,
            }
        );

        // Empty second section produces rma: None
        assert_eq!(
            classify_scan("AFNLFBF||EXTRA"),
            ScanRoute::NoBoxNoLabel {
                payload: "AFNLFBF||EXTRA".to_string(),
                rma: None,
            }
        );

        // Priority 3: AFNLF (not AFNLFBF)
        assert_eq!(
            classify_scan("AFNLF|RMA999|EXTRA"),
            ScanRoute::CustomerPacked {
                payload: "AFNLF|RMA999|EXTRA".to_string(),
                rma: Some("RMA999".to_string()),
            }
        );

        // Caps lock customer packed
        assert_eq!(
            classify_scan("afnlf|rMa999|eXtRa"),
            ScanRoute::CustomerPacked {
                payload: "AFNLF|RmA999|ExTrA".to_string(),
                rma: Some("RmA999".to_string()),
            }
        );

        // Mixed-case prefix starting with lowercase 'a' gets flipped
        assert_eq!(normalize_caps_lock("aFNLF|x"), "Afnlf|X");
        // Prefix starting with uppercase 'A' is NOT flipped even if remaining letters are lowercase
        assert_eq!(normalize_caps_lock("Afnlf|x"), "Afnlf|x");
    }

    #[test]
    fn unpk_and_1z_routing_and_caps_lock_inversion() {
        assert_eq!(
            classify_scan("UNPK-ABC-123"),
            ScanRoute::Unpacked {
                payload: "UNPK-ABC-123".to_string(),
                content: "ABC-123".to_string(),
            }
        );

        assert_eq!(
            classify_scan("unpk-aBc-123"),
            ScanRoute::Unpacked {
                payload: "UNPK-AbC-123".to_string(),
                content: "AbC-123".to_string(),
            }
        );

        assert_eq!(
            classify_scan("1Z12E33Y3451233145"),
            ScanRoute::UpsTracking {
                payload: "1Z12E33Y3451233145".to_string(),
            }
        );

        assert_eq!(
            classify_scan("1z12e33y3451233145"),
            ScanRoute::UpsTracking {
                payload: "1Z12E33Y3451233145".to_string(),
            }
        );

        // Already uppercase "1Z" preserves mixed trailing case
        assert_eq!(normalize_caps_lock("1ZAbC"), "1ZAbC");
        assert_eq!(
            classify_scan("12345"),
            ScanRoute::Unrecognized {
                payload: "12345".to_string(),
            }
        );
    }
}
