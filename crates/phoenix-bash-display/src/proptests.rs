//! Property-based tests for the private `paths_match` path-comparison helper.
//!
//! `paths_match` is the leaf that decides whether a `cd` target refers to the
//! same directory as the conversation's cwd. It runs against adversarial `cd`
//! targets emitted by an LLM, so panic-freedom on arbitrary input is the
//! headline property.
//!
//! All filesystem-dependent properties are kept deterministic by generating
//! absolute paths under a root that cannot exist. `canonicalize()` then always
//! fails and the function falls back to pure `Path` comparison — so these tests
//! do not depend on real filesystem state.

use super::*;
use proptest::prelude::*;

/// A definitely-nonexistent absolute root, so `canonicalize()` always fails and
/// `paths_match` exercises its string/`Path` fallback deterministically.
const NONEXISTENT_ROOT: &str = "/phoenix_nonexistent_zzz";

/// Generate a nonexistent absolute path with 1..=4 lowercase segments under
/// `NONEXISTENT_ROOT`, never with a trailing slash.
fn arb_nonexistent_abs_path() -> impl Strategy<Value = String> {
    proptest::collection::vec("[a-z]{1,8}", 1..=4)
        .prop_map(|segs| format!("{NONEXISTENT_ROOT}/{}", segs.join("/")))
}

proptest! {
    /// TOTALITY: for fully arbitrary `target` and `cwd` strings (empty, unicode,
    /// `~` prefixes, embedded control/separator chars), `paths_match` must return
    /// a bool and never panic. This is the load-bearing property — it proves
    /// panic-freedom on adversarial `cd` targets.
    #[test]
    fn prop_paths_match_never_panics(target in ".*", cwd in ".*") {
        let _: bool = paths_match(&target, &cwd);
    }

    /// ABSOLUTE REFLEXIVITY: any nonexistent absolute path matches itself.
    #[test]
    fn prop_absolute_reflexive(p in arb_nonexistent_abs_path()) {
        prop_assert!(paths_match(&p, &p), "path must match itself: {p}");
    }

    /// TRAILING-SLASH INVARIANCE: a trailing slash on either side is ignored by
    /// `Path` comparison, so `p` and `p/` are interchangeable.
    #[test]
    fn prop_trailing_slash_invariant(p in arb_nonexistent_abs_path()) {
        let with_slash = format!("{p}/");
        prop_assert!(
            paths_match(&p, &with_slash),
            "target {p} should match cwd {with_slash}"
        );
        prop_assert!(
            paths_match(&with_slash, &p),
            "target {with_slash} should match cwd {p}"
        );
    }

    /// TILDE ALWAYS FALSE: any `~`-prefixed target never matches a cwd. This
    /// crate does not expand `~` (it has no access to the resolved home), so
    /// every tilde path — `~`, `~/x`, `~user` — is reported as not-matching.
    #[test]
    fn prop_tilde_user_never_matches(
        user in "[a-z]{1,8}",
        cwd in arb_nonexistent_abs_path(),
    ) {
        let target = format!("~{user}");
        prop_assert!(
            !paths_match(&target, &cwd),
            "~user target {target} must never match cwd {cwd}"
        );
    }
}
