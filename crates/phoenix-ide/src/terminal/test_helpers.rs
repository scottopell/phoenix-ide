//! Builder for ANSI/OSC byte sequences used in terminal tests.
//!
//! `cfg(test)` only — not compiled into production binaries.

/// Builder for constructing ANSI/OSC byte sequences used in tests.
#[allow(dead_code)]
pub struct TerminalStream {
    bytes: Vec<u8>,
}

#[allow(dead_code)]
impl TerminalStream {
    /// Create an empty stream builder.
    pub fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Append OSC 133;A (prompt start).
    pub fn osc133_a(mut self) -> Self {
        self.bytes.extend_from_slice(b"\x1b]133;A\x07");
        self
    }

    /// Append OSC 133;B (command start / end of prompt).
    pub fn osc133_b(mut self) -> Self {
        self.bytes.extend_from_slice(b"\x1b]133;B\x07");
        self
    }

    /// Append OSC 133;C;{cmd} (command executed, with command text).
    pub fn osc133_c(mut self, cmd: &str) -> Self {
        self.bytes.extend_from_slice(b"\x1b]133;C;");
        self.bytes.extend_from_slice(cmd.as_bytes());
        self.bytes.push(0x07);
        self
    }

    /// Append OSC 133;D or OSC 133;D;{n} (command finished).
    ///
    /// `code = None` emits `\x1b]133;D\x07` (no exit code).
    /// `code = Some(n)` emits `\x1b]133;D;{n}\x07`.
    pub fn osc133_d(mut self, code: Option<i32>) -> Self {
        match code {
            None => {
                self.bytes.extend_from_slice(b"\x1b]133;D\x07");
            }
            Some(n) => {
                self.bytes.extend_from_slice(b"\x1b]133;D;");
                self.bytes.extend_from_slice(n.to_string().as_bytes());
                self.bytes.push(0x07);
            }
        }
        self
    }

    /// Append OSC 133;C;{cmd} using ST terminator (`\x1b\\`) instead of BEL.
    ///
    /// Real shells (zsh, fish) often use ST. Behaviour must be identical to BEL variant.
    pub fn osc133_c_st(mut self, cmd: &str) -> Self {
        self.bytes.extend_from_slice(b"\x1b]133;C;");
        self.bytes.extend_from_slice(cmd.as_bytes());
        self.bytes.extend_from_slice(b"\x1b\\"); // ST
        self
    }

    /// Append OSC 133;D or OSC 133;D;{n} using ST terminator.
    pub fn osc133_d_st(mut self, code: Option<i32>) -> Self {
        match code {
            None => self.bytes.extend_from_slice(b"\x1b]133;D\x1b\\"),
            Some(n) => {
                self.bytes.extend_from_slice(b"\x1b]133;D;");
                self.bytes.extend_from_slice(n.to_string().as_bytes());
                self.bytes.extend_from_slice(b"\x1b\\");
            }
        }
        self
    }

    /// Append OSC 7 CWD notification: `\x1b]7;file://localhost{path}\x07`.
    pub fn osc7(mut self, path: &str) -> Self {
        self.bytes.extend_from_slice(b"\x1b]7;file://localhost");
        self.bytes.extend_from_slice(path.as_bytes());
        self.bytes.push(0x07);
        self
    }

    /// Append raw text bytes (no escaping).
    pub fn text(mut self, s: &str) -> Self {
        self.bytes.extend_from_slice(s.as_bytes());
        self
    }

    /// Append an SGR sequence: `\x1b[{code}m`.
    pub fn sgr(mut self, code: u8) -> Self {
        self.bytes.push(0x1b);
        self.bytes.push(b'[');
        self.bytes.extend_from_slice(code.to_string().as_bytes());
        self.bytes.push(b'm');
        self
    }

    /// Append a raw CSI sequence: `\x1b[{seq}`.
    pub fn csi(mut self, seq: &str) -> Self {
        self.bytes.push(0x1b);
        self.bytes.push(b'[');
        self.bytes.extend_from_slice(seq.as_bytes());
        self
    }

    /// Consume the builder and return the accumulated bytes.
    pub fn build(self) -> Vec<u8> {
        self.bytes
    }
}

/// Shortcut: build a complete command sequence (OSC 133;C + output + OSC 133;D).
///
/// Equivalent to:
/// ```text
/// TerminalStream::new().osc133_c(cmd).text(output).osc133_d(code).build()
/// ```
pub fn full_command(cmd: &str, output: &str, code: Option<i32>) -> Vec<u8> {
    TerminalStream::new()
        .osc133_c(cmd)
        .text(output)
        .osc133_d(code)
        .build()
}
