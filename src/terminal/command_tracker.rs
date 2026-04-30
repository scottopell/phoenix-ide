//! Server-side command tracker — REQ-TERM-021.
//!
//! `CommandTracker` implements `vte::Perform` and maintains a ring buffer of
//! at most 5 completed `CommandRecord` entries, keyed by OSC 133 C/D markers.
//!
//! See `specs/terminal/design.md §Command Tracker` and `terminal.allium` for
//! the authoritative behavioural contract.

use std::collections::VecDeque;
use std::time::SystemTime;

use vte::{Params, Perform};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Ring buffer capacity (REQ-TERM-021).
const RING_CAPACITY: usize = 5;

/// Output truncation threshold in bytes — matches bash tool `MAX_OUTPUT_LENGTH` (REQ-TERM-021).
const MAX_OUTPUT: usize = 128 * 1024;

/// Preview size stored in the record when output is truncated.
const TRUNCATED_PREVIEW: usize = 4 * 1024;

// ── CommandRecord ─────────────────────────────────────────────────────────────

/// A single completed shell command tracked via OSC 133 C/D markers.
///
/// Populated at C (`started_at`, `command_text`), finalised at D (`output`, `exit_code`,
/// `duration_ms`).  `output` is ANSI-stripped; if it exceeded `MAX_OUTPUT` it
/// contains a truncated preview plus a disk path annotation.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    /// OSC 133;C payload (may be empty string when the shell doesn't populate it).
    pub command_text: String,
    /// Captured text between C and D, ANSI stripped.
    /// When truncated: first 4 KB + `\n[output truncated; full output at: {path}]`.
    pub output: String,
    /// Optional exit code from OSC 133;D payload.  `None` when D omits the code.
    /// Never substituted with 0 — absence is meaningful.
    pub exit_code: Option<i32>,
    /// Wall-clock time when the C marker was processed.
    pub started_at: SystemTime,
    /// Milliseconds elapsed from C to D.
    pub duration_ms: u64,
}

// ── CaptureState ─────────────────────────────────────────────────────────────

struct CaptureState {
    command_text: String,
    started_at: SystemTime,
}

// ── CommandTracker ────────────────────────────────────────────────────────────

/// OSC 133 command tracker.
///
/// Feed every PTY output byte via `ingest` (REQ-TERM-010).  When capturing
/// (between a C and D marker), printable characters and newlines accumulate
/// in `output_buffer`.  ANSI stripping is structural: `vte` only calls `print`
/// for printable characters; escape sequences never reach it.
pub struct CommandTracker {
    records: VecDeque<CommandRecord>,
    current_capture: Option<CaptureState>,
    output_buffer: String,
    session_id: String,
    seq: u64,
    parser: vte::Parser,
}

impl CommandTracker {
    /// Create a new tracker for the given terminal session.
    ///
    /// `session_id` is used to construct the disk path for truncated outputs
    /// (`~/.phoenix-ide/terminal-output/<session_id>/<seq>.txt`).
    pub fn new(session_id: String) -> Self {
        Self {
            records: VecDeque::with_capacity(RING_CAPACITY),
            current_capture: None,
            output_buffer: String::new(),
            session_id,
            seq: 0,
            // vte::Parser::new() uses a Vec-backed OSC buffer (unbounded) when the
            // `std` feature is active (the default). Do NOT add `default-features = false`
            // to the vte dep — that switches to a 1024-byte fixed buffer and silently
            // truncates long command texts in osc_dispatch.
            parser: vte::Parser::new(),
        }
    }

    /// Feed raw PTY bytes into the tracker.
    ///
    /// Must be called with EVERY byte read from the PTY master fd, in order,
    /// with no gaps (REQ-TERM-010 / `CommandTrackerFedEveryByte` invariant).
    pub fn ingest(&mut self, bytes: &[u8]) {
        // vte::Parser::advance requires &mut self AND &mut Perform simultaneously,
        // which the borrow checker disallows when both are the same struct. Swap
        // the parser out, advance it against self-as-Perform, then swap back.
        //
        // Exception safety: if a Perform method panics, self.parser is left as a
        // fresh Parser::new() (the old one is on the stack and gets dropped). In
        // this implementation no Perform method can panic — disk writes in
        // finalize_output use let-_ to swallow errors — so this is safe in practice.
        let mut parser = std::mem::replace(&mut self.parser, vte::Parser::new());
        parser.advance(self, bytes);
        self.parser = parser;
    }

    /// Return the most recently completed command, if any.
    pub fn last_command(&self) -> Option<&CommandRecord> {
        self.records.back()
    }

    /// Return up to `count` recent commands, newest first.
    ///
    /// Clamps `count` to `min(count, RING_CAPACITY)`.  Never panics on an
    /// empty buffer.
    pub fn recent_commands(&self, count: usize) -> Vec<&CommandRecord> {
        let n = count.min(RING_CAPACITY).min(self.records.len());
        self.records.iter().rev().take(n).collect()
    }

    /// How many records are currently in the ring buffer.
    #[cfg(test)]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Whether a capture is currently in progress (between C and D).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn is_capturing(&self) -> bool {
        self.current_capture.is_some()
    }

    /// All completed records, oldest first.
    #[cfg(test)]
    pub fn all_records(&self) -> &VecDeque<CommandRecord> {
        &self.records
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn handle_osc133_c(&mut self, command_text: String) {
        self.current_capture = Some(CaptureState {
            command_text,
            started_at: SystemTime::now(),
        });
        self.output_buffer.clear();
    }

    fn handle_osc133_d(&mut self, exit_code: Option<i32>) {
        let Some(state) = self.current_capture.take() else {
            tracing::debug!("OSC 133;D received with no current_command; ignoring");
            return;
        };

        let duration_ms = state
            .started_at
            .elapsed()
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        let output = self.finalize_output();
        self.output_buffer.clear();

        let record = CommandRecord {
            command_text: state.command_text,
            output,
            exit_code,
            started_at: state.started_at,
            duration_ms,
        };

        if self.records.len() >= RING_CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(record);
        self.seq += 1;
    }

    /// Finalise the output buffer, applying truncation if needed.
    ///
    /// If `output_buffer.len() > MAX_OUTPUT`:
    /// - Write full content to `~/.phoenix-ide/terminal-output/<session>/<seq>.txt`
    /// - Return first 4 KB + `\n[output truncated; full output at: {path}]`
    fn finalize_output(&self) -> String {
        if self.output_buffer.len() <= MAX_OUTPUT {
            return self.output_buffer.clone();
        }

        let disk_path = self.disk_path_for_seq();

        // Write full output to disk.
        if let Some(parent) = std::path::Path::new(&disk_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&disk_path, self.output_buffer.as_bytes());

        // Build the truncated preview: split at a valid UTF-8 char boundary at or
        // before TRUNCATED_PREVIEW bytes.
        let raw = self.output_buffer.as_str();
        let cutoff = TRUNCATED_PREVIEW.min(raw.len());
        // Walk backward to find a char boundary — guaranteed to succeed at 0.
        let safe_end = (0..=cutoff)
            .rev()
            .find(|&i| raw.is_char_boundary(i))
            .unwrap_or(0);
        // SAFETY: safe_end is guaranteed to be a valid char boundary by the check above.
        #[allow(clippy::string_slice)]
        let preview = &raw[..safe_end];
        format!("{preview}\n[output truncated; full output at: {disk_path}]")
    }

    fn disk_path_for_seq(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!(
            "{}/.phoenix-ide/terminal-output/{}/{}.txt",
            home, self.session_id, self.seq
        )
    }
}

// ── vte::Perform ──────────────────────────────────────────────────────────────

impl Perform for CommandTracker {
    /// Called for every printable character.
    ///
    /// `vte` guarantees this is never called for escape sequences — ANSI
    /// stripping is therefore structural, not a post-processing step.
    fn print(&mut self, c: char) {
        if self.current_capture.is_some() {
            self.output_buffer.push(c);
        }
    }

    /// Called for C0/C1 control bytes.
    ///
    /// We only care about LF, CR, and TAB when capturing.  All other control
    /// bytes are ignored — they are either cursor-movement sequences (not
    /// relevant to text content) or already handled by other Perform methods.
    fn execute(&mut self, byte: u8) {
        if self.current_capture.is_some() {
            match byte {
                0x0a => self.output_buffer.push('\n'), // LF
                0x0d => self.output_buffer.push('\r'), // CR
                0x09 => self.output_buffer.push('\t'), // TAB
                _ => {}
            }
        }
    }

    /// Called for OSC sequences.
    ///
    /// `params[0]` is the numeric part before the first `;`.
    /// For OSC 133: params = `[b"133", b"C" | b"D" | b"A" | b"B", ...]`
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() || params[0] != b"133" {
            return;
        }
        let marker = match params.get(1) {
            Some(m) => *m,
            None => return,
        };
        // A and B are no-ops (PromptBoundaryMarkerAccepted rule); merged with wildcard
        // to satisfy match_same_arms. The _ arm covers unknown markers.
        match marker {
            b"C" => {
                // vte splits the OSC payload on every ';', so a command like
                // `for i in 1 2 3; do echo $i; done` arrives as params[2]="for i in 1 2 3",
                // params[3]=" do echo $i", params[4]=" done". Reassemble params[2..] with
                // ';' to recover the full command text.
                let cmd = params[2..]
                    .iter()
                    .map(|b| std::str::from_utf8(b).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(";");
                self.handle_osc133_c(cmd);
            }
            b"D" => {
                let code = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .and_then(|s| s.parse::<i32>().ok());
                self.handle_osc133_d(code);
            }
            _ => {
                // A and B: accepted, no state change (PromptBoundaryMarkerAccepted).
                // Unknown markers: silently ignored.
            }
        }
    }

    // All remaining Perform methods are intentional no-ops.
    // CommandTracker has no grid, no cursor, and no resize state.

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn csi_dispatch(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::test_helpers::{full_command, TerminalStream};

    fn tracker() -> CommandTracker {
        CommandTracker::new("test-session".to_string())
    }

    #[test]
    fn single_command_recorded() {
        let mut t = tracker();
        t.ingest(&full_command("ls", "file.txt\n", Some(0)));
        let rec = t.last_command().expect("should have a record");
        assert_eq!(rec.command_text, "ls");
        assert_eq!(rec.output, "file.txt\n");
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn exit_code_none_when_d_omits_it() {
        let mut t = tracker();
        t.ingest(&full_command("ls", "", None));
        let rec = t.last_command().unwrap();
        assert_eq!(
            rec.exit_code, None,
            "exit_code must be None when D omits code"
        );
    }

    #[test]
    fn ansi_stripped_from_output() {
        let mut t = tracker();
        // Inject SGR codes between text — they must not appear in output.
        let bytes = TerminalStream::new()
            .osc133_c("echo hi")
            .sgr(1) // bold
            .text("hi")
            .sgr(0) // reset
            .osc133_d(Some(0))
            .build();
        t.ingest(&bytes);
        let rec = t.last_command().unwrap();
        assert_eq!(rec.output, "hi", "ANSI escape codes must be stripped");
    }

    #[test]
    fn ring_buffer_evicts_oldest_at_capacity() {
        let mut t = tracker();
        for i in 0..6 {
            t.ingest(&full_command(
                &format!("cmd{i}"),
                &format!("out{i}\n"),
                Some(i),
            ));
        }
        assert_eq!(t.record_count(), RING_CAPACITY);
        // cmd0 should be evicted; cmd1 is now oldest.
        let recent = t.recent_commands(5);
        assert!(
            !recent.iter().any(|r| r.command_text == "cmd0"),
            "oldest entry should be evicted"
        );
    }

    #[test]
    fn d_without_c_is_noop() {
        let mut t = tracker();
        // Stray D with no preceding C.
        let bytes = TerminalStream::new().osc133_d(Some(0)).build();
        t.ingest(&bytes);
        assert_eq!(t.record_count(), 0);
    }

    #[test]
    fn recent_commands_newest_first() {
        let mut t = tracker();
        t.ingest(&full_command("first", "a", Some(0)));
        t.ingest(&full_command("second", "b", Some(0)));
        let cmds = t.recent_commands(2);
        assert_eq!(cmds[0].command_text, "second");
        assert_eq!(cmds[1].command_text, "first");
    }

    #[test]
    fn recent_commands_clamped_to_capacity() {
        let mut t = tracker();
        t.ingest(&full_command("x", "y", Some(0)));
        // Asking for more than available.
        let cmds = t.recent_commands(100);
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn split_delivery_produces_same_result() {
        let bytes = full_command("ls", "file.txt\n", Some(0));
        // Deliver in two halves.
        let mid = bytes.len() / 2;
        let mut t = tracker();
        t.ingest(&bytes[..mid]);
        t.ingest(&bytes[mid..]);
        let rec = t.last_command().expect("should have a record");
        assert_eq!(rec.command_text, "ls");
        assert_eq!(rec.output, "file.txt\n");
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn ab_markers_are_noops() {
        let mut t = tracker();
        let bytes = TerminalStream::new().osc133_a().osc133_b().build();
        t.ingest(&bytes);
        assert_eq!(t.record_count(), 0, "A/B markers must not create records");
    }

    #[test]
    fn cr_lf_tab_in_output() {
        let mut t = tracker();
        let bytes = TerminalStream::new()
            .osc133_c("cat")
            .text("line1\r\nline2\ttab")
            .osc133_d(Some(0))
            .build();
        t.ingest(&bytes);
        let rec = t.last_command().unwrap();
        assert!(rec.output.contains('\r'), "CR should be preserved");
        assert!(rec.output.contains('\n'), "LF should be preserved");
        assert!(rec.output.contains('\t'), "TAB should be preserved");
    }

    // ── Battle tests ─────────────────────────────────────────────────────────────

    #[test]
    fn st_terminated_osc_works_like_bel() {
        // Real shells (zsh, fish) use ST (\x1b\) not BEL (\x07).
        let mut t = tracker();
        let bytes = TerminalStream::new()
            .osc133_c_st("make")
            .text("built\n")
            .osc133_d_st(Some(0))
            .build();
        t.ingest(&bytes);
        let rec = t
            .last_command()
            .expect("ST-terminated OSC must produce a record");
        assert_eq!(rec.command_text, "make");
        assert_eq!(rec.output, "built\n");
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn c_during_capture_resets_capture() {
        // A second C while already capturing should overwrite; the first command's
        // output is lost (OneExecutingCommandAtATime invariant — outer lifecycle lost).
        let mut t = tracker();
        let bytes = TerminalStream::new()
            .osc133_c("outer")
            .text("outer-output\n")
            .osc133_c("inner") // arrives before outer's D
            .text("inner-output\n")
            .osc133_d(Some(0))
            .build();
        t.ingest(&bytes);
        // Only one record: the inner command.
        assert_eq!(t.record_count(), 1);
        let rec = t.last_command().unwrap();
        assert_eq!(rec.command_text, "inner");
        assert_eq!(rec.output, "inner-output\n");
    }

    #[test]
    fn byte_by_byte_delivery() {
        let bytes = full_command("git status", "On branch main\n", Some(0));
        let mut t = tracker();
        for byte in &bytes {
            t.ingest(std::slice::from_ref(byte));
        }
        let rec = t
            .last_command()
            .expect("byte-by-byte delivery must produce a record");
        assert_eq!(rec.command_text, "git status");
        assert_eq!(rec.output, "On branch main\n");
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn negative_exit_code() {
        // Shells can report signal-killed processes as negative codes (e.g. -1, -11).
        let bytes = TerminalStream::new()
            .osc133_c("yes")
            .osc133_d(Some(-1))
            .build();
        let mut t = tracker();
        t.ingest(&bytes);
        let rec = t.last_command().unwrap();
        assert_eq!(rec.exit_code, Some(-1));
    }

    #[test]
    fn unparseable_exit_code_becomes_none() {
        // Garbage in the D payload (not a valid i32) must not panic; exit_code = None.
        let raw = b"\x1b]133;D;not-a-number\x07";
        let mut t = tracker();
        // Seed a capture first.
        t.ingest(&TerminalStream::new().osc133_c("cmd").build());
        t.ingest(raw);
        let rec = t.last_command().unwrap();
        assert_eq!(
            rec.exit_code, None,
            "unparseable exit code must become None"
        );
    }

    #[test]
    fn c_with_no_cmd_text_param() {
        // Some shells emit \x1b]133;C\x07 with no trailing semicolon or text.
        // params = [b"133", b"C"] — params.get(2) is None — command_text = "".
        let bytes = b"\x1b]133;C\x07output here\x1b]133;D;0\x07";
        let mut t = tracker();
        t.ingest(bytes);
        let rec = t
            .last_command()
            .expect("C with no text param must still produce record");
        assert_eq!(
            rec.command_text, "",
            "missing cmd param must yield empty string"
        );
        assert_eq!(rec.output, "output here");
    }

    #[test]
    fn utf8_boundary_safe_in_truncation() {
        // Build output that puts a multi-byte char (€ = 3 bytes) straddling the 4KB preview
        // cutoff point to verify finalize_output walks back to a valid char boundary.
        let mut t = CommandTracker::new("utf8-test".to_string());

        // Seed capture.
        t.ingest(&TerminalStream::new().osc133_c("cat").build());

        // Fill output_buffer to just below TRUNCATED_PREVIEW with ASCII, then append €.
        // TRUNCATED_PREVIEW = 4096. We want € bytes to straddle byte 4096.
        // € is 3 bytes: 0xE2 0x82 0xAC. Place first byte at 4095 → straddles 4096.
        let padding_len = 4095; // bytes before €
        let padding = "a".repeat(padding_len); // all ASCII, 1 byte each
                                               // Output buffer total: padding_len + 3 (€) + fill to exceed MAX_OUTPUT.
                                               // We need len > 128*1024 to trigger truncation.
        let filler = "b".repeat(MAX_OUTPUT - padding_len); // pushes well past threshold
        let full_output = format!("{padding}€{filler}");

        // Directly poke the output_buffer by ingesting bytes that vte will forward to print().
        // Use text() which emits raw bytes; vte will call print() for valid UTF-8 chars.
        let mut stream = TerminalStream::new();
        // Append the full output text as raw bytes.
        stream = stream.text(&full_output);
        t.ingest(&stream.build());

        // Finalize via D.
        t.ingest(&TerminalStream::new().osc133_d(Some(0)).build());

        let rec = t.last_command().expect("truncated record must be present");
        // The output must be valid UTF-8 (no panic from slicing mid-char).
        assert!(std::str::from_utf8(rec.output.as_bytes()).is_ok());
        assert!(rec.output.contains("[output truncated; full output at:"));
    }

    #[test]
    fn empty_output_command() {
        let mut t = tracker();
        t.ingest(&full_command("true", "", Some(0)));
        let rec = t.last_command().unwrap();
        assert_eq!(rec.output, "");
        assert_eq!(rec.exit_code, Some(0));
    }

    #[test]
    fn interleaved_ansi_and_text() {
        // Simulate realistic terminal output: prompt-reset SGR + cursor movement + text.
        let mut t = tracker();
        let bytes = TerminalStream::new()
            .osc133_c("cargo build")
            .sgr(0) // reset
            .csi("2K") // erase line
            .csi("1;32m") // bold green
            .text("Compiling foo\n")
            .sgr(0)
            .csi("1A") // cursor up — must not appear in output
            .text("   Finished\n")
            .osc133_d(Some(0))
            .build();
        t.ingest(&bytes);
        let rec = t.last_command().unwrap();
        // Only printable text, no ANSI.
        assert!(!rec.output.contains('\x1b'), "output must not contain ESC");
        assert!(rec.output.contains("Compiling foo"));
        assert!(rec.output.contains("Finished"));
    }
}
