//! Subprocess invocation helpers for the tmux tool dispatch.
//!
//! REQ-TMUX-010 (output limits, cancellation, timeout), REQ-TMUX-012
//! (output capture format).

/// Default `wait_seconds` for an agent tmux call (REQ-TMUX-010 /
/// `tmux_tool_default_wait_seconds`).
pub const TMUX_TOOL_DEFAULT_WAIT_SECONDS: u64 = 30;

/// Upper bound on `wait_seconds` for one tmux tool call
/// (REQ-TMUX-010 / `tmux_tool_max_wait_seconds`).
pub const TMUX_TOOL_MAX_WAIT_SECONDS: u64 = 900;

/// Maximum combined stdout+stderr before middle-truncation (REQ-TMUX-010
/// / `tmux_output_max_bytes`). 128 KB.
pub const TMUX_OUTPUT_MAX_BYTES: usize = 128 * 1024;

/// Bytes preserved from each end on truncation (REQ-TMUX-010 /
/// `tmux_truncation_keep_bytes`). 4 KB.
pub const TMUX_TRUNCATION_KEEP_BYTES: usize = 4096;

/// Truncate a single byte stream to `max_bytes` using middle-truncation.
/// If `bytes.len() <= max_bytes`, returns the bytes verbatim as a
/// `String` (lossy UTF-8 conversion).
///
/// Otherwise emits `<head>\n[output truncated in middle: got N, kept
/// K+K]\n<tail>` where K is `TMUX_TRUNCATION_KEEP_BYTES` capped at
/// `max_bytes / 2`.
fn truncate_middle(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let keep = TMUX_TRUNCATION_KEEP_BYTES.min(max_bytes / 2);
    if keep == 0 {
        return format!(
            "[output truncated in middle: got {}, kept 0+0]",
            bytes.len()
        );
    }
    let head = &bytes[..keep];
    let tail = &bytes[bytes.len() - keep..];
    format!(
        "{}\n[output truncated in middle: got {}, kept {}+{}]\n{}",
        String::from_utf8_lossy(head),
        bytes.len(),
        keep,
        keep,
        String::from_utf8_lossy(tail),
    )
}

/// Apply combined-budget middle-truncation to a stdout/stderr pair
/// (REQ-TMUX-010, design.md "Output Capture and Truncation").
///
/// If the combined size is within budget, returns both streams verbatim
/// and `truncated=false`. Otherwise allocates half of `TMUX_OUTPUT_MAX_BYTES`
/// to stdout and the remainder to stderr, applying middle-truncation to
/// whichever stream is over budget.
pub fn truncate_pair(stdout: &[u8], stderr: &[u8]) -> (String, String, bool) {
    let total = stdout.len() + stderr.len();
    if total <= TMUX_OUTPUT_MAX_BYTES {
        return (
            String::from_utf8_lossy(stdout).into_owned(),
            String::from_utf8_lossy(stderr).into_owned(),
            false,
        );
    }

    let budget_each = TMUX_OUTPUT_MAX_BYTES / 2;
    let so = truncate_middle(stdout, budget_each);
    // The stderr budget is whatever the stdout truncation didn't use,
    // so a small stdout doesn't artificially constrain stderr.
    let stderr_budget = TMUX_OUTPUT_MAX_BYTES.saturating_sub(so.len());
    let se = truncate_middle(stderr, stderr_budget);

    (so, se, true)
}

// Tool dispatch and terminal attach reach `spawn_session` and
// `ensure_live` directly via `super::registry`; the design.md called
// out a thin `invoke` module path but the actual primitives live in
// `registry.rs` because they share state with the per-conversation
// `TmuxServer` entries.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_middle_no_truncation_when_under_budget() {
        let s = b"hello world";
        let out = truncate_middle(s, 100);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn truncate_middle_truncates_when_over_budget() {
        let s = vec![b'a'; 50_000];
        let out = truncate_middle(&s, 1000);
        assert!(out.contains("[output truncated in middle: got 50000"));
        // Head and tail of 500 bytes each: total ≤ 1000 + marker.
        assert!(out.starts_with(&"a".repeat(500)));
        assert!(out.ends_with(&"a".repeat(500)));
    }

    #[test]
    fn truncate_pair_under_budget_passes_through() {
        let (so, se, t) = truncate_pair(b"abc", b"def");
        assert_eq!(so, "abc");
        assert_eq!(se, "def");
        assert!(!t);
    }

    #[test]
    fn truncate_pair_over_budget_marks_truncated() {
        let stdout = vec![b'x'; 200_000];
        let stderr = vec![b'y'; 200_000];
        let (so, se, t) = truncate_pair(&stdout, &stderr);
        assert!(t);
        // Combined size after truncation must be reasonable; the marker
        // makes the strings slightly longer than the keep window.
        assert!(so.len() < 20_000);
        assert!(se.len() < 20_000);
    }

    #[test]
    fn truncate_pair_one_stream_over_budget() {
        let stdout = vec![b'x'; 200_000];
        let stderr = b"warning";
        let (so, se, t) = truncate_pair(&stdout, stderr);
        assert!(t);
        assert_eq!(se, "warning");
        assert!(so.contains("[output truncated in middle"));
    }
}
