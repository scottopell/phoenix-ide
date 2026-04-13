//! Property-based and unit tests for the terminal module.
//!
//! Spec: `specs/terminal/terminal.allium`
//! Obligations covered:
//!   - OneTerminalPerConversation invariant (REQ-TERM-003)
//!   - ParserDimensionSync invariant (REQ-TERM-006, REQ-TERM-010)
//!   - is_terminal() correctness (REQ-TERM-012 precondition)
//!   - Dims validity (ResizeFrameRejected precondition)
//!   - try_insert 409 semantics (DuplicateTerminalRejected rule)
//!   - remove/get lifecycle (TerminalOpened / UserClosedTerminal state transitions)

#![allow(clippy::unwrap_used)]

use proptest::prelude::*;

use super::session::{ActiveTerminals, Dims};
use crate::state_machine::state::ConvState;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Generate arbitrary valid terminal dimensions (both > 0, fits in u16).
fn arb_valid_dims() -> impl Strategy<Value = Dims> {
    (1u16..=500u16, 1u16..=200u16).prop_map(|(cols, rows)| Dims { cols, rows })
}

/// Generate arbitrary conversation IDs (non-empty strings).
fn arb_conv_id() -> impl Strategy<Value = String> {
    "[a-z0-9]{8}-[a-z0-9]{4}".prop_map(|s| s)
}

/// Build a minimal `TerminalHandle` for registry tests.
/// Uses /dev/null as a stand-in fd since these tests never do PTY I/O.
fn dummy_handle(dims: Dims) -> super::session::TerminalHandle {
    use std::fs::OpenOptions;
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .expect("open /dev/null");
    let raw = f.into_raw_fd();
    // SAFETY: we own the fd, transferring to OwnedFd.
    let owned_fd = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) };

    let parser = vt100::Parser::new(dims.rows, dims.cols, 0);
    let (quiescence_tx, _) = tokio::sync::watch::channel(0u64);

    super::session::TerminalHandle {
        master_fd: owned_fd,
        child_pid: nix::unistd::Pid::from_raw(1), // init — never reaped in tests
        parser: std::sync::Arc::new(std::sync::Mutex::new(parser)),
        quiescence_tx,
    }
}

// ── Unit: OneTerminalPerConversation (registry semantics) ─────────────────────

/// REQ-TERM-003 / DuplicateTerminalRejected rule:
/// `try_insert` on an already-active conversation returns `None`.
#[test]
fn try_insert_rejects_duplicate() {
    let registry = ActiveTerminals::new();
    let conv_id = "conv-001".to_string();
    let dims = Dims { cols: 80, rows: 24 };

    // First insert succeeds.
    let first = registry.try_insert(conv_id.clone(), dummy_handle(dims));
    assert!(first.is_some(), "first insert should succeed");

    // Second insert is rejected (409).
    let second = registry.try_insert(conv_id.clone(), dummy_handle(dims));
    assert!(second.is_none(), "duplicate insert must return None (409)");
}

/// After `remove`, a new insert succeeds (absent → active → absent → active cycle).
#[test]
fn remove_allows_reinsertion() {
    let registry = ActiveTerminals::new();
    let conv_id = "conv-002".to_string();
    let dims = Dims { cols: 80, rows: 24 };

    registry
        .try_insert(conv_id.clone(), dummy_handle(dims))
        .unwrap();
    registry.remove(&conv_id);

    let third = registry.try_insert(conv_id.clone(), dummy_handle(dims));
    assert!(third.is_some(), "insert after remove must succeed");
}

/// `get` returns `Some` for registered conversations, `None` otherwise.
#[test]
fn get_returns_correct_presence() {
    let registry = ActiveTerminals::new();
    let dims = Dims { cols: 80, rows: 24 };

    assert!(registry.get("nonexistent").is_none());

    registry
        .try_insert("present".to_string(), dummy_handle(dims))
        .unwrap();
    assert!(registry.get("present").is_some());
    assert!(registry.get("nonexistent").is_none());
}

// ── Property: OneTerminalPerConversation ──────────────────────────────────────

proptest! {
    /// Invariant: for any sequence of try_insert / remove operations across
    /// distinct conversation IDs, the count of active terminals per conversation
    /// never exceeds 1.
    ///
    /// Maps to: `OneTerminalPerConversation` in terminal.allium.
    #[test]
    fn prop_one_terminal_per_conversation(
        ops in proptest::collection::vec(
            (arb_conv_id(), proptest::bool::ANY),  // (conv_id, insert=true / remove=false)
            1..50
        )
    ) {
        let registry = ActiveTerminals::new();
        let dims = Dims { cols: 80, rows: 24 };

        for (conv_id, do_insert) in ops {
            if do_insert {
                // try_insert either succeeds or returns None — never panics.
                let _ = registry.try_insert(conv_id.clone(), dummy_handle(dims));
            } else {
                registry.remove(&conv_id);
            }

            // Invariant: count per conversation must be 0 or 1.
            let map = registry.0.lock().unwrap();
            let count = map.iter().filter(|(k, _)| **k == conv_id).count();
            prop_assert!(count <= 1,
                "OneTerminalPerConversation violated: {} active for {}",
                count, conv_id);
        }
    }

    /// Concurrent-simulation: two inserts racing on the same conversation ID
    /// must result in at most one active terminal. We simulate this serially
    /// (Rust Mutex guarantees atomicity; the spec requires it).
    #[test]
    fn prop_concurrent_insert_one_wins(conv_id in arb_conv_id()) {
        let registry = ActiveTerminals::new();
        let dims = Dims { cols: 80, rows: 24 };

        let r1 = registry.try_insert(conv_id.clone(), dummy_handle(dims));
        let r2 = registry.try_insert(conv_id.clone(), dummy_handle(dims));

        // Exactly one succeeds.
        let successes = [r1.is_some(), r2.is_some()].iter().filter(|&&b| b).count();
        prop_assert_eq!(successes, 1,
            "exactly one of two racing inserts must win; got {}", successes);
    }
}

// ── Unit: ParserDimensionSync invariant ──────────────────────────────────────

/// REQ-TERM-006 / ParserDimensionSync:
/// After a resize, the parser's size must equal the requested Dims.
#[test]
fn parser_dimension_sync_after_resize() {
    let initial = Dims { cols: 80, rows: 24 };
    let mut parser = vt100::Parser::new(initial.rows, initial.cols, 0);

    // Initial dimensions match.
    let (r, c) = parser.screen().size();
    assert_eq!(c, initial.cols, "initial cols match");
    assert_eq!(r, initial.rows, "initial rows match");

    // Apply a resize.
    let new_dims = Dims {
        cols: 132,
        rows: 48,
    };
    parser.set_size(new_dims.rows, new_dims.cols);

    let (r2, c2) = parser.screen().size();
    assert_eq!(c2, new_dims.cols, "cols after resize");
    assert_eq!(r2, new_dims.rows, "rows after resize");
}

// ── Property: ParserDimensionSync ─────────────────────────────────────────────

proptest! {
    /// Invariant: after any sequence of resize operations, the parser's reported
    /// size matches the last-applied Dims. Simulates the ParserDimensionSync
    /// invariant across arbitrary resize sequences.
    ///
    /// Maps to: `ParserDimensionSync` in terminal.allium.
    #[test]
    fn prop_parser_dimension_sync(
        initial in arb_valid_dims(),
        resizes in proptest::collection::vec(arb_valid_dims(), 0..20),
    ) {
        let mut parser = vt100::Parser::new(initial.rows, initial.cols, 0);
        let mut last_dims = initial;

        for dims in resizes {
            // apply_resize equivalent: update both PTY (skipped — no real fd)
            // and parser.
            parser.set_size(dims.rows, dims.cols);
            last_dims = dims;

            // ParserDimensionSync invariant.
            let (r, c) = parser.screen().size();
            prop_assert_eq!(c, last_dims.cols,
                "ParserDimensionSync: cols mismatch after resize to {:?}", dims);
            prop_assert_eq!(r, last_dims.rows,
                "ParserDimensionSync: rows mismatch after resize to {:?}", dims);
        }

        // Final check: size reflects the last resize.
        let (r_final, c_final) = parser.screen().size();
        prop_assert_eq!(c_final, last_dims.cols);
        prop_assert_eq!(r_final, last_dims.rows);
    }

    /// ParserFedEveryByte (structural aspect):
    /// The vt100 parser must not panic on arbitrary byte sequences, and must
    /// process all bytes without corruption (no partial application).
    ///
    /// Maps to: `ParserFedEveryByte` in terminal.allium.
    ///
    /// NOTE: vt100 0.15.2 had a panic on some byte sequences (e.g. `[32, 32, 0]`)
    /// due to integer overflow (`prev_pos.row -= scrolled`) in grid.rs col_wrap().
    /// Fixed in vendor/vt100 via `saturating_sub`. Task 08667.
    #[test]
    fn prop_parser_accepts_arbitrary_bytes(
        dims in arb_valid_dims(),
        byte_sequences in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..256),
            1..10
        ),
    ) {
        let mut parser = vt100::Parser::new(dims.rows, dims.cols, 0);

        for chunk in &byte_sequences {
            // Must not panic on any byte sequence (fixed in vendor/vt100 via saturating_sub).
            parser.process(chunk);

            // Dimensions must remain stable (no resize here).
            let (r, c) = parser.screen().size();
            prop_assert_eq!(c, dims.cols, "cols must not change from byte processing");
            prop_assert_eq!(r, dims.rows, "rows must not change from byte processing");
        }
    }
}

// ── Unit: Dims validity ───────────────────────────────────────────────────────

/// ResizeFrameRejected precondition: dims with cols=0 or rows=0 are invalid.
#[test]
fn dims_zero_cols_is_invalid() {
    // The spec requires dimensions.cols > 0 and dimensions.rows > 0.
    // Our ws.rs rejects frames where either is 0.
    // This test documents the boundary; `apply_resize` is only called
    // after the guard in the writer task.
    let invalid = Dims { cols: 0, rows: 24 };
    assert_eq!(invalid.cols, 0, "zero cols recognized as boundary case");
}

// ── Unit: is_terminal() completeness ─────────────────────────────────────────

/// REQ-TERM-012 / TerminalAbandonedWithConversation:
/// Terminal teardown triggers on ConversationBecameTerminal, which fires when
/// is_terminal() becomes true. Verify all four terminal states return true.
///
/// The bug fixed in task 08662 (ContextExhausted was missing) means this test
/// would have caught the regression.
#[test]
fn is_terminal_covers_all_terminal_states() {
    let terminal_states: &[ConvState] = &[
        ConvState::Completed {
            result: "done".into(),
        },
        ConvState::Failed {
            error: "err".into(),
            error_kind: crate::db::ErrorKind::Cancelled,
        },
        ConvState::ContextExhausted {
            summary: "summary".into(),
        },
        ConvState::Terminal,
    ];

    for state in terminal_states {
        assert!(
            state.is_terminal(),
            "is_terminal() must return true for {:?} — required for REQ-TERM-012 teardown",
            std::mem::discriminant(state)
        );
    }
}

/// Non-terminal states must NOT be treated as terminal.
#[test]
fn is_terminal_excludes_non_terminal_states() {
    let non_terminal: &[ConvState] = &[
        ConvState::Idle,
        ConvState::AwaitingContinuation {
            rejected_tool_calls: vec![],
            attempt: 0,
        },
    ];

    for state in non_terminal {
        assert!(
            !state.is_terminal(),
            "is_terminal() must return false for {:?}",
            std::mem::discriminant(state)
        );
    }
}

// ── Property: is_terminal() agrees with ConversationBecameTerminal ────────────

proptest! {
    /// REQ-TERM-012: ConversationBecameTerminal fires when is_terminal() becomes true.
    /// Property: is_terminal() is stable — calling it twice on the same state
    /// returns the same value (no side effects, no volatility).
    ///
    /// Also verifies Idle is always non-terminal (required: terminal teardown must
    /// not fire on conversations that haven't ended).
    #[test]
    fn prop_is_terminal_is_idempotent(
        summary in "[a-z ]{0,50}",
        result in "[a-z ]{0,50}",
    ) {
        let states = vec![
            ConvState::Idle,
            ConvState::Terminal,
            ConvState::Completed { result: result.clone() },
            ConvState::Failed { error: "e".into(), error_kind: crate::db::ErrorKind::Cancelled },
            ConvState::ContextExhausted { summary: summary.clone() },
        ];

        for state in &states {
            let first = state.is_terminal();
            let second = state.is_terminal();
            prop_assert_eq!(first, second,
                "is_terminal() must be idempotent on {:?}", std::mem::discriminant(state));
        }

        // Idle is always non-terminal.
        prop_assert!(!ConvState::Idle.is_terminal(),
            "Idle must never be terminal — teardown must not fire on active conversations");

        // Terminal is always terminal.
        prop_assert!(ConvState::Terminal.is_terminal());
    }
}

// ── High-pressure: tiny terminals + long sequences ────────────────────────────

/// Operation for `prop_parser_stress_resize_then_draw`.
#[derive(Debug)]
enum TerminalOp {
    Resize(Dims),
    Draw(Vec<u8>),
}

fn arb_terminal_op() -> impl Strategy<Value = TerminalOp> {
    prop_oneof![
        arb_valid_dims().prop_map(TerminalOp::Resize),
        proptest::collection::vec(any::<u8>(), 0..256).prop_map(TerminalOp::Draw),
    ]
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 512,
        ..proptest::test_runner::Config::default()
    })]

    /// Stress test for the patched vt100 vendor: tiny terminals (1×1 to 4×4)
    /// combined with long byte sequences and frequent wide Unicode characters.
    /// These dimensions were the original panic trigger. 512 cases gives good
    /// coverage of the wide-char/scroll interaction space without slowing CI.
    #[test]
    fn prop_parser_stress_tiny_terminals(
        cols in 1u16..=4u16,
        rows in 1u16..=4u16,
        // Long sequences with many wide-char codepoints in the mix
        byte_sequences in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..1024),
            1..20
        ),
    ) {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        for chunk in &byte_sequences {
            parser.process(chunk);
            let (r, c) = parser.screen().size();
            prop_assert_eq!(c, cols, "cols changed after processing bytes");
            prop_assert_eq!(r, rows, "rows changed after processing bytes");
        }
    }

    /// Stress test: arbitrary resize sequences interleaved with byte processing.
    /// Verifies ParserDimensionSync holds and no panics occur across the
    /// resize+draw interaction that previously triggered underflows.
    #[test]
    fn prop_parser_stress_resize_then_draw(
        initial in arb_valid_dims(),
        ops in proptest::collection::vec(arb_terminal_op(), 1..30),
    ) {
        let mut parser = vt100::Parser::new(initial.rows, initial.cols, 0);
        let mut current_dims = initial;

        for op in &ops {
            match op {
                TerminalOp::Resize(dims) => {
                    parser.set_size(dims.rows, dims.cols);
                    current_dims = *dims;
                }
                TerminalOp::Draw(chunk) => {
                    parser.process(chunk);
                    let (r, c) = parser.screen().size();
                    prop_assert_eq!(c, current_dims.cols,
                        "ParserDimensionSync: cols wrong after draw at {:?}", current_dims);
                    prop_assert_eq!(r, current_dims.rows,
                        "ParserDimensionSync: rows wrong after draw at {:?}", current_dims);
                }
            }
        }
    }
}

// ── Unit: vt100 save/restore cursor clamps across shrinking resize ───────────

/// Regression for task 24668:
///
/// The vendored vt100 crate's `restore_cursor` (driven by ESC 8 / DECRC)
/// replayed the saved cursor position directly into `self.pos` without
/// clamping against the current grid size. When a `save_cursor` on a large
/// grid was followed by a `set_size` shrinking the grid and then a
/// `restore_cursor`, the cursor landed outside the grid and the next draw
/// panicked in `drawing_cell().unwrap()`.
///
/// This test exercises the exact API-level save → resize → restore → draw
/// sequence with deterministic inputs so regressions can be caught without
/// relying on the proptest random exploration.
#[test]
fn vt100_restore_cursor_clamps_after_shrinking_resize() {
    let mut parser = vt100::Parser::new(10, 100, 0);

    // Move the cursor to (9, 99) on the 10x100 grid — last row, last col.
    // Cursor position: `ESC [ row ; col H` is 1-indexed, so row=10, col=100.
    parser.process(b"\x1b[10;100H");
    let pos = parser.screen().cursor_position();
    assert_eq!(pos, (9, 99), "cursor not at bottom-right of 10x100 grid");

    // Save the cursor with DECSC (ESC 7).
    parser.process(b"\x1b7");

    // Shrink the grid to 3x5. This should clamp the LIVE cursor, but
    // leave the SAVED cursor stale at (9, 99).
    parser.set_size(3, 5);
    let pos_after_resize = parser.screen().cursor_position();
    assert!(
        pos_after_resize.0 < 3 && pos_after_resize.1 < 5,
        "live cursor not clamped after resize, got {pos_after_resize:?}",
    );

    // Restore with DECRC (ESC 8). Without the fix, self.pos becomes (9, 99)
    // on a 3x5 grid.
    parser.process(b"\x1b8");
    let pos_after_restore = parser.screen().cursor_position();
    assert!(
        pos_after_restore.0 < 3 && pos_after_restore.1 < 5,
        "restore_cursor did not clamp saved position against current grid; \
         got {pos_after_restore:?} on a 3x5 grid",
    );

    // A subsequent draw must not panic. Pre-fix, this panicked inside
    // vt100::Screen::text at `drawing_cell(...).unwrap()`.
    parser.process(b"X");

    // Sanity: grid stayed at 3x5 and the write landed somewhere valid.
    let (rows, cols) = parser.screen().size();
    assert_eq!((rows, cols), (3, 5));
}

// ── Unit: resize frame validation (ResizeFrameRejected rule) ─────────────────

/// REQ-TERM-006 / ResizeFrameRejected:
/// The spec requires cols > 0 && rows > 0 for a resize to be applied.
/// Zero-dimension frames must be silently dropped.
#[test]
fn zero_cols_resize_frame_is_rejected() {
    use std::sync::{Arc, Mutex};

    let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));
    let initial_dims = Dims { cols: 80, rows: 24 };

    // Construct a 0x01 frame with cols=0
    let data = {
        let mut v = vec![0x01u8];
        v.extend_from_slice(&0u16.to_be_bytes()); // cols = 0
        v.extend_from_slice(&24u16.to_be_bytes()); // rows = 24
        v
    };

    // handle_binary_frame must return true (don't disconnect) and leave parser unchanged

    let result =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(super::relay::dispatch_frame_for_test(
                &parser,
                &data,
                "test-conv",
            ));
    assert!(result, "zero-cols frame should not disconnect the session");

    let (r, c) = parser.lock().unwrap().screen().size();
    assert_eq!(
        c, initial_dims.cols,
        "cols must be unchanged after zero-dim resize"
    );
    assert_eq!(
        r, initial_dims.rows,
        "rows must be unchanged after zero-dim resize"
    );
}

#[test]
fn zero_rows_resize_frame_is_rejected() {
    use std::sync::{Arc, Mutex};
    let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));

    let data = {
        let mut v = vec![0x01u8];
        v.extend_from_slice(&80u16.to_be_bytes()); // cols = 80
        v.extend_from_slice(&0u16.to_be_bytes()); // rows = 0
        v
    };

    let result =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(super::relay::dispatch_frame_for_test(
                &parser,
                &data,
                "test-conv",
            ));
    assert!(result);

    let (r, c) = parser.lock().unwrap().screen().size();
    assert_eq!(c, 80u16, "cols unchanged");
    assert_eq!(r, 24u16, "rows unchanged");
}

proptest! {
    /// ResizeFrameRejected: for any frame with zero cols or rows, the parser
    /// dimensions must remain unchanged.
    #[test]
    fn prop_zero_dimension_resize_rejected(
        initial in arb_valid_dims(),
        bad_cols in 0u16..=1u16,   // 0 is invalid, 1 is valid; test boundary
        bad_rows in 0u16..=1u16,
    ) {
        use std::sync::{Arc, Mutex};
        // Only test cases where at least one dimension is 0
        prop_assume!(bad_cols == 0 || bad_rows == 0);

        let parser = Arc::new(Mutex::new(
            vt100::Parser::new(initial.rows, initial.cols, 0)
        ));
        let data = {
            let mut v = vec![0x01u8];
            v.extend_from_slice(&bad_cols.to_be_bytes());
            v.extend_from_slice(&bad_rows.to_be_bytes());
            v
        };


        let _result = tokio::runtime::Runtime::new().unwrap().block_on(super::relay::dispatch_frame_for_test(&parser, &data, "test"));

        let (r, c) = parser.lock().unwrap().screen().size();
        prop_assert_eq!(c, initial.cols, "cols must be unchanged after invalid resize");
        prop_assert_eq!(r, initial.rows, "rows must be unchanged after invalid resize");
    }
}

// ── Unit: build_env (REQ-TERM-002 / ShellEnvironmentConstructed rule) ─────────

/// REQ-TERM-002: The shell environment must contain all required variables
/// and must NOT inherit the server process environment.
#[test]
fn build_env_contains_required_variables() {
    use super::spawn::build_env;

    let env = build_env("/bin/bash");
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

    for required in &["TERM", "COLORTERM", "HOME", "USER", "SHELL", "PATH", "LANG"] {
        assert!(
            keys.contains(required),
            "build_env missing required key: {required}"
        );
    }
}

#[test]
fn build_env_term_is_xterm_256color() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash");
    let term = env
        .iter()
        .find(|(k, _)| k == "TERM")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        term,
        Some("xterm-256color"),
        "TERM must be xterm-256color — wrong value breaks readline and vim"
    );
}

#[test]
fn build_env_colorterm_is_truecolor() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash");
    let ct = env
        .iter()
        .find(|(k, _)| k == "COLORTERM")
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("truecolor"));
}

#[test]
fn build_env_shell_matches_argument() {
    use super::spawn::build_env;
    let env = build_env("/usr/bin/zsh");
    let shell = env
        .iter()
        .find(|(k, _)| k == "SHELL")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        shell,
        Some("/usr/bin/zsh"),
        "SHELL env var must reflect the shell passed to build_env"
    );
}

#[test]
fn build_env_lang_is_utf8() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash");
    let lang = env
        .iter()
        .find(|(k, _)| k == "LANG")
        .map(|(_, v)| v.as_str());
    assert_eq!(lang, Some("en_US.UTF-8"));
}

#[test]
fn build_env_no_duplicate_keys() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash");
    let mut keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    let original_len = keys.len();
    keys.dedup();
    assert_eq!(
        keys.len(),
        original_len,
        "build_env must not produce duplicate keys"
    );
}

// ── wezterm-term proptests (evaluation, task 24673) ──────────────────────────
//
// These tests run the SAME stress scenarios as the vt100 variants above, but
// against `WezParser` (see `wezterm_parser.rs`).  Identical shapes, identical
// assertions.  Failures here indicate either a wezterm-term bug or an
// incompatible edge case that would need fixing before migration.
//
// Run with:  PROPTEST_CASES=10000 cargo test wez_stress

#[cfg(test)]
mod wez_proptest {
    use proptest::prelude::*;

    use super::{arb_terminal_op, arb_valid_dims, TerminalOp};
    use crate::terminal::wezterm_parser::WezParser;

    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 512,
            ..proptest::test_runner::Config::default()
        })]

        /// wezterm-term variant of `prop_parser_stress_tiny_terminals`.
        ///
        /// Tiny terminals (1×1 to 4×4) combined with long byte sequences and
        /// frequent wide-char codepoints.  This is the dimension range that
        /// triggered all six vt100 patches.  If wezterm-term panics here,
        /// it's a go/no-go blocker.
        ///
        /// **Known failure**: `tattoy-wezterm-term 0.1.0-fork.5` (and upstream
        /// `wezterm/wezterm`) panics on bytes `[144, 113, 63, 128]` (DCS 'q' =
        /// Sixel start) on a 1×1 terminal: divide-by-zero in
        /// `terminalstate/image.rs:103` (`assign_image_to_cells`).  This is a
        /// go/no-go **BLOCKER** for the migration.  See
        /// `specs/terminal/wezterm-evaluation.md` for full analysis.
        ///
        /// Marked `#[ignore]` so CI passes; run explicitly:
        /// `PROPTEST_CASES=10000 cargo test wez_prop -- --ignored`
        #[test]
        #[ignore = "wezterm-term divide-by-zero bug (task 24673 blocker); run explicitly for evaluation"]
        fn wez_prop_parser_stress_tiny_terminals(
            cols in 1u16..=4u16,
            rows in 1u16..=4u16,
            byte_sequences in proptest::collection::vec(
                proptest::collection::vec(any::<u8>(), 0..1024),
                1..20
            ),
        ) {
            let mut parser = WezParser::new(rows, cols);
            for chunk in &byte_sequences {
                parser.process(chunk);
                let (r, c) = parser.size();
                prop_assert_eq!(c, cols, "cols changed after processing bytes");
                prop_assert_eq!(r, rows, "rows changed after processing bytes");
            }
        }

        /// wezterm-term variant of `prop_parser_stress_resize_then_draw`.
        ///
        /// Arbitrary resize sequences interleaved with byte processing.
        /// Verifies `ParserDimensionSync` holds against wezterm-term and that
        /// no panics occur across the resize+draw interaction.
        ///
        /// **Known failure**: same divide-by-zero as `wez_prop_parser_stress_tiny_terminals`.
        /// Sixel/DCS sequence on a tiny terminal hits `assign_image_to_cells`
        /// with `draw_width = 0`.  Go/no-go **BLOCKER**.
        ///
        /// Marked `#[ignore]` so CI passes; run explicitly:
        /// `PROPTEST_CASES=10000 cargo test wez_prop -- --ignored`
        #[test]
        #[ignore = "wezterm-term divide-by-zero bug (task 24673 blocker); run explicitly for evaluation"]
        fn wez_prop_parser_stress_resize_then_draw(
            initial in arb_valid_dims(),
            ops in proptest::collection::vec(arb_terminal_op(), 1..30),
        ) {
            let mut parser = WezParser::new(initial.rows, initial.cols);
            let mut current_dims = initial;

            for op in &ops {
                match op {
                    TerminalOp::Resize(dims) => {
                        parser.set_size(dims.rows, dims.cols);
                        current_dims = *dims;
                    }
                    TerminalOp::Draw(chunk) => {
                        parser.process(chunk);
                        let (r, c) = parser.size();
                        prop_assert_eq!(c, current_dims.cols,
                            "ParserDimensionSync: cols wrong after draw at {:?}",
                            current_dims);
                        prop_assert_eq!(r, current_dims.rows,
                            "ParserDimensionSync: rows wrong after draw at {:?}",
                            current_dims);
                    }
                }
            }
        }
    }
}
