//! Per-handle byte-bounded ring buffer for bash tool output.
//!
//! REQ-BASH-004: Ring Buffer and Read Semantics.
//!
//! Read accessors (`bytes_cap`, `is_empty`, etc.) are exercised by task
//! 02694's `BashTool` peek/wait response shapers; they read as dead in
//! foundation-only builds.
#![allow(dead_code)]
//!
//! Incoming bytes are split on `\n` into [`RingLine`]s, each carrying a
//! monotonically increasing `offset` (line index since spawn). When the
//! ring's accumulated bytes exceed `bytes_cap`, oldest lines are evicted
//! and `start_offset` advances. Offsets are NEVER reused — eviction
//! discards lines, but every line's offset remains a valid identifier
//! for "the i-th line emitted by this handle's process."

use std::collections::VecDeque;

/// Default per-handle ring buffer cap (REQ-BASH-004: `RING_BUFFER_BYTES`).
pub const RING_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// One line in the ring buffer or tombstone tail.
///
/// `bytes` holds the line content WITHOUT the trailing `\n`. Lossy UTF-8
/// conversion at the API boundary preserves binary payloads as
/// replacement-character runs (per the Allium spec's `RingLine.bytes`
/// description).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingLine {
    /// Monotonic line index since spawn (>= 0). Never reused.
    pub offset: u64,
    /// Raw line bytes without the trailing newline.
    pub bytes: Vec<u8>,
}

/// Byte-bounded ring buffer with per-line monotonic offsets.
#[derive(Debug)]
pub struct RingBuffer {
    /// Complete lines, oldest-first. Offsets strictly increasing within the deque.
    lines: VecDeque<RingLine>,
    /// A trailing partial line (no `\n` seen yet). Will be flushed either
    /// when its `\n` arrives or on EOF via [`Self::flush_partial`].
    partial: Vec<u8>,
    /// Total bytes accounted for in `lines` (sum of `line.bytes.len()`).
    /// Newline overhead is NOT counted here — eviction is by line content
    /// bytes, which matches the spec's "per-handle live ring buffer
    /// bounded by `RING_BUFFER_BYTES`" intent (the cap is on captured
    /// content, not on framing).
    bytes_used: usize,
    /// Cap on `bytes_used` before eviction triggers.
    bytes_cap: usize,
    /// Offset of the oldest still-retained line, or the offset that the
    /// next-oldest line will have if the ring is currently empty.
    start_offset: u64,
    /// Offset to assign to the next complete line.
    next_offset: u64,
}

impl RingBuffer {
    /// Create an empty ring with the given byte cap.
    pub fn new(bytes_cap: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            partial: Vec::new(),
            bytes_used: 0,
            bytes_cap,
            start_offset: 0,
            next_offset: 0,
        }
    }

    /// Total bytes currently retained in `lines`.
    pub fn bytes_used(&self) -> usize {
        self.bytes_used
    }

    /// Offset of the oldest retained line. If the ring is empty, the
    /// offset that an immediate-next-flushed line would receive.
    pub fn start_offset(&self) -> u64 {
        self.start_offset
    }

    /// Offset assigned to the next line. Equivalently: one past the
    /// offset of the most recently appended line.
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }

    /// Number of lines currently retained.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// True if no lines are retained (partial buffer is irrelevant here).
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Cap on retained bytes. Lines are evicted when `bytes_used > bytes_cap`.
    pub fn bytes_cap(&self) -> usize {
        self.bytes_cap
    }

    /// Append the given bytes to the ring. Splits on `\n`; complete lines
    /// receive sequential offsets, a trailing partial chunk is held in
    /// `self.partial` until its `\n` arrives or `flush_partial` is called.
    ///
    /// After appending, evicts oldest lines until `bytes_used <= bytes_cap`.
    /// The most recently appended line is never evicted by this call —
    /// even a single line larger than `bytes_cap` is retained (with
    /// `start_offset` advanced past every prior line). This matches
    /// `RingLineEvicted`'s `count(handle.ring_lines) > 1` guard.
    pub fn append(&mut self, mut data: &[u8]) {
        while let Some(nl) = memchr(b'\n', data) {
            let (head, rest) = data.split_at(nl);
            // head ... = bytes before '\n', no '\n' at end
            let mut line_bytes = std::mem::take(&mut self.partial);
            line_bytes.extend_from_slice(head);
            self.push_line(line_bytes);
            // skip the '\n' itself
            data = &rest[1..];
        }
        if !data.is_empty() {
            self.partial.extend_from_slice(data);
        }
        self.evict_to_cap();
    }

    /// Flush any held partial bytes as a final line. Used on EOF when the
    /// child terminated without a trailing newline. No-op if `partial`
    /// is empty.
    pub fn flush_partial(&mut self) {
        if !self.partial.is_empty() {
            let line_bytes = std::mem::take(&mut self.partial);
            self.push_line(line_bytes);
            self.evict_to_cap();
        }
    }

    fn push_line(&mut self, bytes: Vec<u8>) {
        let line = RingLine {
            offset: self.next_offset,
            bytes,
        };
        self.bytes_used += line.bytes.len();
        self.next_offset += 1;
        self.lines.push_back(line);
    }

    fn evict_to_cap(&mut self) {
        // Spec: drop oldest while bytes_used > cap AND more than one line
        // remains. A single oversize line is kept (any read returns it
        // with truncated_before=true if the requested view extends below
        // the new start_offset).
        while self.bytes_used > self.bytes_cap && self.lines.len() > 1 {
            let evicted = self
                .lines
                .pop_front()
                .expect("len > 1 implies pop_front is Some");
            self.bytes_used -= evicted.bytes.len();
            // start_offset advances to the new oldest line's offset.
            // After pop_front, lines.front() is the new oldest (the deque
            // had at least 2 lines).
            self.start_offset = self.lines.front().map_or(self.next_offset, |l| l.offset);
        }
    }

    /// Read the last `n` lines (or all lines if fewer than `n` exist).
    /// Returns the slice and a `truncated_before` flag computed from the
    /// caller's frame of reference (true if the slice does not include
    /// `start_offset`'s line — i.e. lines have been evicted that the
    /// caller might have wanted).
    ///
    /// `truncated_before` for tail-mode reads: true iff at least one
    /// line has ever been evicted (i.e. `start_offset > 0`) AND the
    /// slice does not begin at `start_offset`. Practically: the caller
    /// asked for the tail and the ring has dropped older content that
    /// is not in the slice. This matches REQ-BASH-004's "eviction
    /// occurred since the agent's prior peek (tail mode)" — for the
    /// first peek there is no "prior peek," but the agent should still
    /// see the bit set if eviction has happened so they know content
    /// fell out before they observed it.
    pub fn tail(&self, n: usize) -> WindowView {
        let total = self.lines.len();
        let take = n.min(total);
        let skip = total - take;
        let lines: Vec<RingLine> = self.lines.iter().skip(skip).cloned().collect();
        let view_start = lines.first().map_or(self.next_offset, |l| l.offset);
        // Tail-mode truncation rule (REQ-BASH-004): true iff eviction
        // has dropped any line — i.e. `start_offset > 0`. When the
        // caller asks for the tail of an un-evicted ring, the n-line
        // slice is just a window into intact content, NOT truncation
        // ("content I didn't ask for" is not the same as "content the
        // ring lost"). Once eviction has happened, any tail read is
        // by definition served from a ring with content missing
        // before it, and the agent must be told.
        let truncated_before = self.start_offset > 0;
        WindowView {
            start_offset: view_start,
            end_offset: self.next_offset,
            truncated_before,
            lines,
        }
    }

    /// Read lines with offset in `[max(since, start_offset), end_offset)`.
    /// `truncated_before` is true iff the caller's `since` was older than
    /// `start_offset` (REQ-BASH-004 incremental-mode rule).
    pub fn since(&self, since: u64) -> WindowView {
        let effective_start = since.max(self.start_offset);
        let lines: Vec<RingLine> = self
            .lines
            .iter()
            .filter(|l| l.offset >= effective_start)
            .cloned()
            .collect();
        let view_start = lines.first().map_or(effective_start, |l| l.offset);
        WindowView {
            start_offset: view_start,
            end_offset: self.next_offset,
            truncated_before: since < self.start_offset,
            lines,
        }
    }

    /// Snapshot the last `n` lines (or all) without modifying the ring.
    /// Used at exit-time to build the tombstone tail.
    pub fn snapshot_tail(&self, n: usize) -> Vec<RingLine> {
        let total = self.lines.len();
        let take = n.min(total);
        let skip = total - take;
        self.lines.iter().skip(skip).cloned().collect()
    }
}

/// Read result over a ring or tombstone tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowView {
    pub start_offset: u64,
    pub end_offset: u64,
    pub truncated_before: bool,
    pub lines: Vec<RingLine>,
}

/// Tiny memchr stand-in to avoid pulling the `memchr` crate just for one byte.
/// `bash` output line splitting is not the bottleneck.
fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(offset: u64, bytes: &[u8]) -> RingLine {
        RingLine {
            offset,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn split_lines_on_newline_and_assign_offsets() {
        let mut r = RingBuffer::new(1024);
        r.append(b"alpha\nbeta\ngamma\n");
        assert_eq!(r.len(), 3);
        assert_eq!(r.next_offset(), 3);
        assert_eq!(r.start_offset(), 0);
        let view = r.since(0);
        assert_eq!(
            view.lines,
            vec![line(0, b"alpha"), line(1, b"beta"), line(2, b"gamma")]
        );
        assert!(!view.truncated_before);
    }

    #[test]
    fn partial_line_held_until_newline() {
        let mut r = RingBuffer::new(1024);
        r.append(b"hello, ");
        assert_eq!(r.len(), 0); // no complete line yet
        r.append(b"world\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r.since(0).lines, vec![line(0, b"hello, world")]);
    }

    #[test]
    fn flush_partial_emits_trailing_unterminated_line() {
        let mut r = RingBuffer::new(1024);
        r.append(b"no-newline");
        r.flush_partial();
        assert_eq!(r.len(), 1);
        assert_eq!(r.since(0).lines, vec![line(0, b"no-newline")]);
    }

    #[test]
    fn empty_lines_get_their_own_offset() {
        let mut r = RingBuffer::new(1024);
        r.append(b"a\n\nb\n");
        let view = r.since(0);
        assert_eq!(view.lines, vec![line(0, b"a"), line(1, b""), line(2, b"b")]);
    }

    #[test]
    fn eviction_advances_start_offset_byte_bounded() {
        // cap of 8 bytes; pushing 4 lines of 4 bytes each -> 16 bytes ->
        // must evict until <= 8 (and we keep at least one line).
        let mut r = RingBuffer::new(8);
        r.append(b"AAAA\nBBBB\nCCCC\nDDDD\n");
        assert!(r.bytes_used() <= 8);
        assert_eq!(r.next_offset(), 4);
        // We dropped enough to fit; the latest two should remain.
        assert_eq!(r.start_offset(), 2);
        let view = r.since(0);
        assert!(view.truncated_before, "since=0 < start_offset=2");
        assert_eq!(view.lines, vec![line(2, b"CCCC"), line(3, b"DDDD")]);
    }

    #[test]
    fn eviction_keeps_one_line_when_single_line_exceeds_cap() {
        // A single 100-byte line in a 8-byte ring is retained — the
        // eviction loop's "more than one line remains" guard prevents
        // dropping the only line.
        let mut r = RingBuffer::new(8);
        let big: Vec<u8> = b"X".repeat(100);
        let mut data = big.clone();
        data.push(b'\n');
        r.append(&data);
        assert_eq!(r.len(), 1);
        assert_eq!(r.since(0).lines, vec![line(0, &big)]);
        // A second line should evict the first now.
        r.append(b"second\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r.start_offset(), 1);
        let view = r.tail(10);
        assert!(view.truncated_before);
        assert_eq!(view.lines, vec![line(1, b"second")]);
    }

    #[test]
    fn offsets_are_monotonic_across_eviction() {
        let mut r = RingBuffer::new(16);
        for i in 0..50_u64 {
            r.append(format!("line-{i}\n").as_bytes());
        }
        // All retained offsets are strictly increasing.
        let view = r.since(r.start_offset());
        let offsets: Vec<u64> = view.lines.iter().map(|l| l.offset).collect();
        let mut prev = None;
        for o in &offsets {
            if let Some(p) = prev {
                assert!(*o > p, "offsets must be strictly increasing");
            }
            prev = Some(*o);
        }
        // next_offset == 50 (we wrote 50 lines), start_offset > 0 (we evicted some).
        assert_eq!(r.next_offset(), 50);
        assert!(r.start_offset() > 0);
    }

    #[test]
    fn tail_truncated_before_iff_eviction_dropped_content_below_view() {
        // Without eviction, tail of N never sets truncated_before.
        let mut r = RingBuffer::new(1024);
        for i in 0..5 {
            r.append(format!("L{i}\n").as_bytes());
        }
        let v = r.tail(3);
        assert!(!v.truncated_before);
        assert_eq!(v.lines.len(), 3);
        assert_eq!(v.start_offset, 2);
        assert_eq!(v.end_offset, 5);

        // With eviction below the view, truncated_before is true.
        let mut r = RingBuffer::new(8);
        for i in 0..10 {
            r.append(format!("LL{i}\n").as_bytes()); // 4 bytes each line
        }
        let v = r.tail(2);
        assert!(v.truncated_before);
    }

    #[test]
    fn since_truncated_before_when_since_is_below_start_offset() {
        let mut r = RingBuffer::new(8);
        for i in 0..10 {
            r.append(format!("LL{i}\n").as_bytes());
        }
        // start_offset > 0 due to eviction.
        let v = r.since(0);
        assert!(v.truncated_before);
        let v_after = r.since(r.start_offset());
        assert!(!v_after.truncated_before);
    }

    #[test]
    fn since_at_or_past_end_offset_returns_empty_view() {
        let mut r = RingBuffer::new(1024);
        r.append(b"only-line\n");
        let v = r.since(r.next_offset()); // since == end_offset
        assert!(v.lines.is_empty());
        assert!(!v.truncated_before);
        assert_eq!(v.start_offset, v.end_offset);
    }

    #[test]
    fn snapshot_tail_does_not_modify_ring() {
        let mut r = RingBuffer::new(1024);
        r.append(b"a\nb\nc\n");
        let snap = r.snapshot_tail(2);
        assert_eq!(snap, vec![line(1, b"b"), line(2, b"c")]);
        // ring unchanged
        assert_eq!(r.len(), 3);
    }
}
