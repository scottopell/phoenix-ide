//! Terminal parser — wraps `alacritty_terminal` and `vte::ansi::Processor`
//! to expose the same six-method API surface Phoenix uses.
//!
//! Replaces the vendored `vt100 0.15.2` crate (task 24678). See
//! `specs/terminal/alacritty-evaluation.md` for the full evaluation.
//!
//! ## cols >= 2 invariant
//!
//! `alacritty_terminal` panics when a width-2 Unicode character is fed to a
//! 1-column terminal (upstream bug; task 24676).  The relay layer enforces
//! `cols >= 2` at resize time (`ResizeFrameRejected` rule), so this code
//! path is unreachable in production.
//!
//! ## API surface
//!
//! | vt100 call                          | `alacritty_terminal` equivalent                 |
//! |-------------------------------------|-----------------------------------------------|
//! | `Parser::new(rows, cols, 0)`        | `AlacrittyParser::new(rows, cols)`            |
//! | `parser.process(&bytes)`            | `AlacrittyParser::process(&bytes)`            |
//! | `parser.set_size(rows, cols)`       | `AlacrittyParser::set_size(rows, cols)`       |
//! | `parser.screen().size()`            | `AlacrittyParser::size() -> (rows, cols)`     |
//! | `parser.screen().contents()`        | `AlacrittyParser::contents() -> String`       |
//! | `parser.screen().cursor_position()` | `AlacrittyParser::cursor_pos() -> (row, col)` |
//!
//! ## Structural note
//!
//! `alacritty_terminal` separates parser (`vte::ansi::Processor`) and state
//! (`Term<T>`).  `AlacrittyParser` wraps both so callers see one struct.

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line, Point},
    term::Config,
    vte::ansi,
    Term,
};

// ── Dimensions shim ───────────────────────────────────────────────────────────

/// `TermSize` is `#[cfg(test)]`-only in `alacritty_terminal`, so we provide
/// our own.
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

// ── Parser ────────────────────────────────────────────────────────────────────

/// Terminal parser — drop-in replacement for `vt100::Parser`.
///
/// Wraps `alacritty_terminal::Term<VoidListener>` and an `ansi::Processor`.
pub struct AlacrittyParser {
    term: Term<VoidListener>,
    parser: ansi::Processor,
}

impl AlacrittyParser {
    /// Equivalent of `vt100::Parser::new(rows, cols, 0)`.
    /// Precondition: `cols >= 2` (enforced by relay; see module doc).
    pub fn new(rows: u16, cols: u16) -> Self {
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let term = Term::new(Config::default(), &size, VoidListener);
        let parser = ansi::Processor::new();
        Self { term, parser }
    }

    /// Equivalent of `parser.process(&bytes)`. Never panics given `cols >= 2`.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Equivalent of `parser.set_size(rows, cols)`.
    /// Precondition: `cols >= 2` (relay rejects cols < 2 before calling this).
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.term.resize(TermSize {
            cols: cols as usize,
            rows: rows as usize,
        });
    }

    /// Equivalent of `parser.screen().size() -> (rows, cols)`.
    #[allow(dead_code)] // Used by relay tests and terminal HUD
    pub fn size(&self) -> (u16, u16) {
        #[allow(clippy::cast_possible_truncation)]
        (self.term.screen_lines() as u16, self.term.columns() as u16)
    }

    /// Equivalent of `parser.screen().contents() -> String`.
    ///
    /// Mirrors vt100 contract: lines joined with `\n`, trailing whitespace
    /// trimmed per line, trailing newlines stripped from result.
    pub fn contents(&self) -> String {
        let rows = self.term.screen_lines();
        let cols = self.term.columns();
        if rows == 0 || cols == 0 {
            return String::new();
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let end_line = Line(rows as i32 - 1);
        let start = Point::new(Line(0), Column(0));
        let end = Point::new(end_line, Column(cols - 1));
        let raw = self.term.bounds_to_string(start, end);
        let trimmed: String = raw
            .split('\n')
            .map(|l| l.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        trimmed.trim_end_matches('\n').to_string()
    }

    /// Equivalent of `parser.screen().cursor_position() -> (row, col)`.
    /// Always satisfies `row < rows` and `col < cols` (no deferred-wrap).
    #[allow(dead_code)] // Used by terminal HUD; not yet wired in production
    pub fn cursor_pos(&self) -> (u16, u16) {
        let pt = self.term.grid().cursor.point;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let row = pt.line.0.max(0) as u16;
        #[allow(clippy::cast_possible_truncation)]
        let col = pt.column.0 as u16;
        (row, col)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
        // [0x90, 0x71, 0x3F, 0x80]: Sixel/DCS sequence — divide-by-zero in
        // wezterm-term (task 24673 blocker). alacritty has no sixel path.
        let mut p = AlacrittyParser::new(2, 2);
        p.process(&[0x90, 0x71, 0x3F, 0x80]);
        assert_eq!(p.size(), (2, 2));
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
        let mut p = AlacrittyParser::new(10, 100);
        p.process(b"\x1b[10;100H");
        assert_eq!(p.cursor_pos(), (9, 99));

        p.process(b"\x1b7");
        p.set_size(3, 5);
        let after_resize = p.cursor_pos();
        assert!(after_resize.0 < 3 && after_resize.1 <= 5);

        p.process(b"\x1b8");
        let after_restore = p.cursor_pos();
        assert!(
            after_restore.0 < 3 && after_restore.1 < 5,
            "restored cursor not clamped: {after_restore:?}"
        );

        p.process(b"X");
        assert_eq!(p.size(), (3, 5));
    }

    #[test]
    fn alac_osc133_sequences_do_not_panic() {
        let mut p = AlacrittyParser::new(24, 80);
        p.process(b"\x1b]133;A\x07");
        p.process(b"\x1b]133;B\x07");
        p.process(b"\x1b]133;C\x07");
        p.process(b"\x1b]133;D;0\x07");
        assert_eq!(p.size(), (24, 80));
    }

    /// min-cols-2 invariant: verify that wide Unicode on the minimum (cols=2)
    /// terminal does NOT panic. This is the regression guard for the upstream
    /// alacritty bug (task 24676) that panics at cols=1.
    #[test]
    fn alac_wide_char_at_min_cols_does_not_panic() {
        let mut p = AlacrittyParser::new(4, 2);
        // U+3000 IDEOGRAPHIC SPACE (width=2) — the blocker byte sequence
        p.process(&[0xE3, 0x80, 0x80]);
        assert_eq!(p.size(), (4, 2));
    }
}
