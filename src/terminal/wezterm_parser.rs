//! Proof-of-concept adapter: wezterm-term as a drop-in for vt100::Parser.
//!
//! This module is **evaluation only** — it is never called from production code.
//! It exists to prove API parity and run the stress proptests against
//! wezterm-term's implementation. See `specs/terminal/wezterm-evaluation.md`
//! for the full findings.
//!
//! ## API surface covered
//!
//! | vt100 call                      | wezterm-term equivalent               |
//! |---------------------------------|---------------------------------------|
//! | `Parser::new(rows, cols, 0)`    | `WezParser::new(rows, cols)`          |
//! | `parser.process(&bytes)`        | `WezParser::process(&bytes)`          |
//! | `parser.set_size(rows, cols)`   | `WezParser::set_size(rows, cols)`     |
//! | `parser.screen().size()`        | `WezParser::size() -> (rows, cols)`   |
//! | `parser.screen().contents()`    | `WezParser::contents() -> String`     |
//! | `parser.screen().cursor_position()` | `WezParser::cursor_pos() -> (row, col)` |
//!
//! ## Gaps found
//!
//! - `Screen::visible_lines()` is `#[cfg(test)]`-gated in both the upstream
//!   `wezterm/wezterm` and the `tattoy-wezterm-term` fork.  Content extraction
//!   requires `screen.lines_in_phys_range(phys_range)` instead.
//! - `vt100::Screen::contents()` trims trailing spaces/newlines.  This adapter
//!   replicates that behaviour via `trim_end()` per line.
//! - `Terminal::new()` requires a `writer: Box<dyn Write + Send>` for PTY
//!   input echo; we pass `std::io::sink()` in the adapter (no PTY in tests).
//! - `TerminalConfiguration::color_palette()` must be implemented; no blanket
//!   default exists.  We provide `MinimalConfig` which returns the default
//!   `ColorPalette`.

#![cfg(test)]
#![allow(dead_code)] // All items here are proof-of-concept, not wired to production.

use std::sync::Arc;

use tattoy_wezterm_term::{
    color::ColorPalette, config::TerminalConfiguration, Terminal, TerminalSize, VisibleRowIndex,
};

// ── Configuration shim ────────────────────────────────────────────────────────

/// Minimal `TerminalConfiguration` for the adapter.  Uses all defaults and
/// provides only what is required by the trait.
#[derive(Debug)]
struct MinimalConfig;

impl TerminalConfiguration for MinimalConfig {
    fn scrollback_size(&self) -> usize {
        0 // mirrors vt100 `scrollback = 0` used in Phoenix
    }
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/// Drop-in replacement for the vt100 `Parser` API surface used by Phoenix.
///
/// Wraps `wezterm-term::Terminal` and exposes exactly the 6 methods Phoenix
/// calls.  No production code is touched.
pub struct WezParser {
    terminal: Terminal,
    current_rows: u16,
    current_cols: u16,
}

impl WezParser {
    /// Equivalent of `vt100::Parser::new(rows, cols, 0)`.
    pub fn new(rows: u16, cols: u16) -> Self {
        let config = Arc::new(MinimalConfig);
        let size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        let terminal = Terminal::new(
            size,
            config,
            "xterm-256color",
            "phoenix-ide",
            Box::new(std::io::sink()),
        );
        Self {
            terminal,
            current_rows: rows,
            current_cols: cols,
        }
    }

    /// Equivalent of `parser.process(&bytes)`.  May not panic on any input.
    pub fn process(&mut self, bytes: &[u8]) {
        self.terminal.advance_bytes(bytes);
    }

    /// Equivalent of `parser.set_size(rows, cols)`.
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.terminal.resize(TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        });
        self.current_rows = rows;
        self.current_cols = cols;
    }

    /// Equivalent of `parser.screen().size() -> (rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        let s = self.terminal.screen();
        (s.physical_rows as u16, s.physical_cols as u16)
    }

    /// Equivalent of `parser.screen().contents() -> String`.
    ///
    /// vt100's `contents()` joins visible rows with `\n`, trimming trailing
    /// whitespace on each line and stripping trailing newlines from the result.
    /// We replicate that contract here.
    pub fn contents(&self) -> String {
        let screen = self.terminal.screen();
        let phys_range = screen.phys_range(&(0..screen.physical_rows as VisibleRowIndex));
        let lines = screen.lines_in_phys_range(phys_range);
        let mut result: String = lines
            .iter()
            .map(|l| l.as_str().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        // Mirror vt100: strip trailing newlines.
        while result.ends_with('\n') {
            result.pop();
        }
        result
    }

    /// Equivalent of `parser.screen().cursor_position() -> (row, col)`.
    pub fn cursor_pos(&self) -> (u16, u16) {
        let c = self.terminal.cursor_pos();
        (c.y as u16, c.x as u16)
    }
}

// ── Unit tests: API parity ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wez_size_matches_initial_dims() {
        let p = WezParser::new(24, 80);
        assert_eq!(p.size(), (24, 80));
    }

    #[test]
    fn wez_size_after_resize() {
        let mut p = WezParser::new(24, 80);
        p.set_size(10, 40);
        assert_eq!(p.size(), (10, 40));
    }

    #[test]
    fn wez_process_does_not_panic_on_arbitrary_bytes() {
        let mut p = WezParser::new(24, 80);
        // The exact byte sequence from task 08667 that panicked vt100 before patching.
        p.process(&[32, 32, 0]);
        assert_eq!(p.size(), (24, 80));
    }

    #[test]
    fn wez_contents_returns_text() {
        let mut p = WezParser::new(24, 80);
        p.process(b"Hello, world!\r\n");
        let c = p.contents();
        assert!(c.contains("Hello, world!"), "contents: {:?}", c);
    }

    #[test]
    fn wez_cursor_pos_advances() {
        let mut p = WezParser::new(24, 80);
        p.process(b"ABC");
        let (row, col) = p.cursor_pos();
        // After printing 3 chars the cursor should be at column 3.
        assert_eq!(col, 3, "cursor column after 3 chars");
        assert_eq!(row, 0, "cursor row stays at 0");
    }

    /// Mirrors the vt100 regression test from task 24668: save cursor at
    /// (9, 99) on a 10x100 grid, shrink to 3x5, restore.
    ///
    /// ## Behavioral difference from vt100
    ///
    /// wezterm-term uses a **deferred-wrap** cursor model where after the last
    /// column in a row, the cursor sits at `col = physical_cols` (one past the
    /// last valid index) and wraps lazily on the next character.  vt100 patches
    /// the saved cursor to be strictly `< cols` on restore.
    ///
    /// Concretely: after `ESC 8` on a 3×5 grid, wezterm-term may restore the
    /// cursor to `(2, 5)` — row 2 (valid), col 5 (= `physical_cols`, deferred
    /// wrap).  vt100 would report `(2, 4)` (clamped to `cols - 1`).
    ///
    /// **Impact on phoenix**: `cursor_position()` exposed to the `read_terminal`
    /// tool may return `col = physical_cols`.  Callers must treat this as
    /// "cursor is at the pending-wrap position at the end of the row".  The
    /// distinction does not affect screen content reading but must be documented
    /// if we migrate.
    ///
    /// **No panic**: wezterm-term does NOT panic in this state — the deferred
    /// wrap is legal in its model and subsequent bytes wrap cleanly.
    #[test]
    fn wez_resize_cursor_deferred_wrap_semantic() {
        let mut p = WezParser::new(10, 100);
        // Move cursor to bottom-right (1-indexed: row=10, col=100)
        p.process(b"\x1b[10;100H");
        let pos = p.cursor_pos();
        assert_eq!(pos, (9, 99), "cursor not at bottom-right of 10x100");

        // Save cursor (DECSC / ESC 7)
        p.process(b"\x1b7");

        // Shrink to 3x5 — live cursor must move into the valid range
        p.set_size(3, 5);
        let after_resize = p.cursor_pos();
        // Row must always be < rows
        assert!(
            after_resize.0 < 3,
            "live cursor row not clamped after resize: {:?}",
            after_resize
        );
        // Col may equal physical_cols (deferred wrap) — that's valid in wezterm-term
        assert!(
            after_resize.1 <= 5,
            "live cursor col exceeds physical_cols after resize: {:?}",
            after_resize
        );

        // Restore saved cursor (DECRC / ESC 8)
        p.process(b"\x1b8");
        let after_restore = p.cursor_pos();
        // Row must be < rows
        assert!(
            after_restore.0 < 3,
            "restored cursor row exceeds rows: {:?}",
            after_restore
        );
        // Col <= physical_cols (deferred-wrap at physical_cols is allowed)
        assert!(
            after_restore.1 <= 5,
            "restored cursor col exceeds physical_cols: {:?}",
            after_restore
        );

        // Critical: subsequent draw must not panic (this was the original crash in vt100)
        p.process(b"X");
        assert_eq!(p.size(), (3, 5), "grid size must be stable after draw");
    }
}
