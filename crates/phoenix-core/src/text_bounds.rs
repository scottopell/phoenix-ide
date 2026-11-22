//! Byte-budgeted string prefixes that never split a UTF-8 scalar.

/// The longest prefix of `s` that is at most `max_bytes` bytes and ends on a
/// char boundary. Returns `s` unchanged when it already fits.
#[must_use]
pub fn utf8_prefix(s: &str, max_bytes: usize) -> &str {
    // `floor_char_boundary` always yields a boundary, so the empty fallback is
    // unreachable; it errs toward over-truncation rather than the whole input.
    s.split_at_checked(s.floor_char_boundary(max_bytes))
        .map_or("", |(prefix, _)| prefix)
}

/// `s` when it fits in `max_bytes`; otherwise its [`utf8_prefix`] of
/// `max_bytes` followed by `marker`. The marker is not counted against the
/// budget.
#[must_use]
pub fn truncate_with_marker(s: &str, max_bytes: usize, marker: &str) -> String {
    if s.len() <= max_bytes {
        s.to_owned()
    } else {
        format!("{}{marker}", utf8_prefix(s, max_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn prefix_backs_off_to_char_boundary() {
        // "日" is 3 bytes: a 4-byte budget lands inside the second char.
        assert_eq!(utf8_prefix("日本語", 4), "日");
        assert_eq!(utf8_prefix("日本語", 6), "日本");
        assert_eq!(utf8_prefix("日本語", 100), "日本語");
        assert_eq!(utf8_prefix("🦀", 3), "");
        assert_eq!(utf8_prefix("", 0), "");
    }

    #[test]
    fn marker_only_when_truncated() {
        assert_eq!(truncate_with_marker("abc", 3, "..."), "abc");
        assert_eq!(truncate_with_marker("abcd", 3, "..."), "abc...");
        assert_eq!(truncate_with_marker("a日b", 2, "…"), "a…");
    }

    proptest! {
        #[test]
        fn prefix_is_longest_bounded_char_prefix(s in "\\PC{0,64}", max in 0usize..300) {
            let p = utf8_prefix(&s, max);
            prop_assert!(p.len() <= max);
            prop_assert!(s.starts_with(p));
            if p.len() < s.len() {
                // The next char would overflow the budget: nothing longer fits.
                let next = s.get(p.len()..).unwrap().chars().next().unwrap();
                prop_assert!(p.len() + next.len_utf8() > max);
            }
        }

        #[test]
        fn truncation_is_identity_within_budget_and_marked_beyond(
            s in "\\PC{0,64}",
            max in 0usize..300,
        ) {
            let out = truncate_with_marker(&s, max, "[cut]");
            if s.len() <= max {
                prop_assert_eq!(out, s);
            } else {
                prop_assert!(out.ends_with("[cut]"));
                prop_assert!(out.len() <= max + "[cut]".len());
                prop_assert!(s.starts_with(out.strip_suffix("[cut]").unwrap()));
            }
        }
    }
}
