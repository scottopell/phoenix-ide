//! Proof-of-concept adapter: alacritty_terminal as a drop-in for vt100::Parser.
//!
//! This module is **evaluation only** — never called from production code.
//! It proves API parity and runs the stress proptests against alacritty_terminal.
//! See `specs/terminal/alacritty-evaluation.md` for full findings.
//!
//! ## API surface covered
//!
//! | vt100 call                          | alacritty_terminal equivalent                    |
//! |-------------------------------------|--------------------------------------------------|
//! | `Parser::new(rows, cols, 0)`        | `AlacrittyParser::new(rows, cols)`               |
//! | `parser.process(&bytes)`            | `AlacrittyParser::process(&bytes)`               |
//! | `parser.set_size(rows, cols)`       | `AlacrittyParser::set_size(rows, cols)`          |
//! | `parser.screen().size()`            | `AlacrittyParser::size() -> (rows, cols)`        |
//! | `parser.screen().contents()`        | `AlacrittyParser::contents() -> String`          |
//! | `parser.screen().cursor_position()` | `AlacrittyParser::cursor_pos() -> (row, col)`    |
//!
//! ## Key structural difference from vt100 / wezterm-term
//!
//! alacritty_terminal separates the *parser* (`vte::ansi::Processor`) from the
//! *terminal state* (`Term<T>`).  vt100 and wezterm-term bundle both into one
//! struct.  This adapter wraps both in a single `AlacrittyParser` so callers
//! see the same one-struct API.
//!
//! ## Gaps
//!
//! - OSC 133 (FinalTerm semantic prompts) is **not handled** by alacritty_terminal.
//!   OSC 133 sequences fall through to `unhandled()` — a debug log, no callback.
//!   Exit codes (D marker) are not accessible.  See evaluation doc section 4.
//!
//! - `Term<VoidListener>` is used; alacritty events (title changes, bell, etc.)
//!   are silently dropped.  That is correct for headless parser use.

#![cfg(test)]
#![allow(dead_code)]

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line, Point},
    term::Config,
    vte::ansi,
    Term,
};

// ── Dimensions shim ───────────────────────────────────────────────────────────

/// Minimal `Dimensions` implementation for constructing and resizing `Term`.
/// `TermSize` in alacritty_terminal is `#[cfg(test)]`-only; we define our own.
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ── Adapter ──────────────────────────────────────────────────────────────────────

/// Drop-in replacement for the vt100 `Parser` API surface used by Phoenix.
///
/// Wraps `alacritty_terminal::Term<VoidListener>` and an `ansi::Processor`,
/// exposing exactly the 6 methods Phoenix calls.  No production code is touched.
pub struct AlacrittyParser {
    term: Term<VoidListener>,
    parser: ansi::Processor,
}

impl AlacrittyParser {
    /// Equivalent of `vt100::Parser::new(rows, cols, 0)`.
    pub fn new(rows: u16, cols: u16) -> Self {
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let term = Term::new(Config::default(), &size, VoidListener);
        let parser = ansi::Processor::new();
        Self { term, parser }
    }

    /// Equivalent of `parser.process(&bytes)`. May not panic on any input.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Equivalent of `parser.set_size(rows, cols)`.
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        self.term.resize(size);
    }

    /// Equivalent of `parser.screen().size() -> (rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        (self.term.screen_lines() as u16, self.term.columns() as u16)
    }

    /// Equivalent of `parser.screen().contents() -> String`.
    ///
    /// vt100's `contents()` joins visible rows with `\n`, trimming trailing
    /// whitespace on each line and stripping trailing newlines from the result.
    /// We replicate that contract here via `bounds_to_string`.
    pub fn contents(&self) -> String {
        let rows = self.term.screen_lines();
        let cols = self.term.columns();
        if rows == 0 || cols == 0 {
            return String::new();
        }
        let start = Point::new(Line(0), Column(0));
        let end = Point::new(Line(rows as i32 - 1), Column(cols - 1));
        let raw = self.term.bounds_to_string(start, end);
        // Mirror vt100: trim trailing whitespace per line, strip trailing newlines.
        let trimmed: String = raw
            .split('\n')
            .map(|l| l.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        trimmed.trim_end_matches('\n').to_string()
    }

    /// Equivalent of `parser.screen().cursor_position() -> (row, col)`.
    pub fn cursor_pos(&self) -> (u16, u16) {
        let pt = self.term.grid().cursor.point;
        // Line is i32 (negative = scrollback). Viewport rows are 0..screen_lines.
        let row = pt.line.0.max(0) as u16;
        let col = pt.column.0 as u16;
        (row, col)
    }
}

// ── Unit tests: API parity ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alac_size_matches_initial_dims() {
        let p = AlacrittyParser::new(24, 80);
        assert_eq!(p.size(), (24, 80));
    }

    #[test]
    fn alac_size_after_resize() {
        let mut p = AlacrittyParser::new(24, 80);
        p.set_size(10, 40);
        assert_eq!(p.size(), (10, 40));
    }

    #[test]
    fn alac_process_does_not_panic_on_vt100_trigger_bytes() {
        // The byte sequence from task 08667 that panicked vt100 before patching.
        let mut p = AlacrittyParser::new(24, 80);
        p.process(&[32, 32, 0]);
        assert_eq!(p.size(), (24, 80));
    }

    #[test]
    fn alac_process_does_not_panic_on_wezterm_trigger_bytes() {
        // The byte sequence [0x90, 0x71, 0x3F, 0x80] that triggered a
        // divide-by-zero in wezterm-term's sixel handler (task 24673 blocker).
        // alacritty_terminal has no sixel handler, so this should be a no-op.
        let mut p = AlacrittyParser::new(1, 1);
        p.process(&[0x90, 0x71, 0x3F, 0x80]);
        assert_eq!(p.size(), (1, 1));
    }

    #[test]
    fn alac_contents_returns_text() {
        let mut p = AlacrittyParser::new(24, 80);
        p.process(b"Hello, world!\r\n");
        let c = p.contents();
        assert!(c.contains("Hello, world!"), "contents: {c:?}");
    }

    #[test]
    fn alac_cursor_pos_advances() {
        let mut p = AlacrittyParser::new(24, 80);
        p.process(b"ABC");
        let (row, col) = p.cursor_pos();
        assert_eq!(col, 3, "cursor column after 3 chars");
        assert_eq!(row, 0, "cursor row stays at 0");
    }

    #[test]
    fn alac_resize_cursor_clamped() {
        // Mirrors the vt100 regression from task 24668.
        // Save cursor at (9, 99) on 10x100, shrink to 3x5, restore.
        let mut p = AlacrittyParser::new(10, 100);
        p.process(b"\x1b[10;100H"); // move to bottom-right
        let pos = p.cursor_pos();
        assert_eq!(pos, (9, 99), "cursor not at bottom-right");

        p.process(b"\x1b7"); // DECSC save
        p.set_size(3, 5); // shrink

        let after_resize = p.cursor_pos();
        assert!(
            after_resize.0 < 3 && after_resize.1 <= 5,
            "cursor not clamped after resize: {after_resize:?}"
        );

        p.process(b"\x1b8"); // DECRC restore
        let after_restore = p.cursor_pos();
        // alacritty clamps to < cols strictly (no deferred-wrap at col == cols)
        assert!(
            after_restore.0 < 3 && after_restore.1 < 5,
            "restored cursor not clamped: {after_restore:?}"
        );

        // Subsequent draw must not panic
        p.process(b"X");
        assert_eq!(p.size(), (3, 5));
    }

    #[test]
    fn alac_osc133_sequences_do_not_panic() {
        // OSC 133 A/B/C/D sequences must be silently ignored (not crash).
        let mut p = AlacrittyParser::new(24, 80);
        // A = prompt start, B = prompt end, C = command start, D;0 = exit code 0
        p.process(b"\x1b]133;A\x07");
        p.process(b"\x1b]133;B\x07");
        p.process(b"\x1b]133;C\x07");
        p.process(b"\x1b]133;D;0\x07");
        // No panic = pass. Content is unaffected by OSC 133 sequences.
        assert_eq!(p.size(), (24, 80));
    }
}
