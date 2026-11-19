//! Pure SSE (Server-Sent Events) line parser.
//!
//! Extracts the framing logic from provider-specific streaming code into a
//! testable, stateful parser. TCP chunks arrive as arbitrary byte slices;
//! the parser reassembles them into complete SSE events.
//!
//! # SSE wire format (subset we support)
//!
//! ```text
//! event: <type>\n
//! data: <payload>\n
//! \n                    ← blank line dispatches the event
//! ```
//!
//! Key contracts modelled by property tests:
//!
//! 1. **Chunk-boundary independence** — splitting the same byte stream at
//!    different points must produce identical events.
//! 2. **Multi-byte UTF-8 safety** — a codepoint split across chunks must not
//!    corrupt or drop data.
//! 3. **Multi-line `data:`** — consecutive `data:` lines before a blank line
//!    are joined with `\n` (per the SSE spec).
//! 4. **`\r\n`, `\n` and bare `\r`** — all line endings work, including a
//!    `\r\n` pair split across chunks.
//! 5. **Trailing whitespace tolerance** — the gateway may pad `data:` lines.

use phoenix_core::sse_lines::{LineSplitter, SseLimitExceeded};

/// A fully assembled SSE event ready for dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or empty if none was sent.
    pub event_type: String,
    /// The `data:` field (multiple `data:` lines joined with `\n`).
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SseDiagnostics {
    pub chunk_count: usize,
    pub bytes_received: usize,
    pub buffered_bytes: usize,
    pub pending_event_type_len: usize,
    pub pending_data_len: usize,
}

/// Largest single SSE line accepted from a provider. Generous, because a
/// completed Responses-API event carries the whole response on one line; it
/// only has to stop unbounded growth.
pub const MAX_SSE_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Largest event (event type plus all data lines) accepted from a provider.
pub const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// Accumulates fields of the event currently being built.
struct EventAssembler {
    event_type: String,
    data_lines: Vec<String>,
    pending_bytes: usize,
    max_event_bytes: usize,
}

impl EventAssembler {
    fn new(max_event_bytes: usize) -> Self {
        Self {
            event_type: String::new(),
            data_lines: Vec::new(),
            pending_bytes: 0,
            max_event_bytes,
        }
    }

    fn push_line(&mut self, line: &str) -> Result<Option<SseEvent>, SseLimitExceeded> {
        if line.is_empty() {
            return Ok(self.dispatch());
        }
        let (name, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match name {
            "data" => {
                self.pending_bytes = self.pending_bytes.saturating_add(value.len());
                self.data_lines.push(value.to_string());
            }
            "event" => {
                self.pending_bytes = self
                    .pending_bytes
                    .saturating_sub(self.event_type.len())
                    .saturating_add(value.len());
                self.event_type = value.to_string();
            }
            // a comment line (empty name) and unknown fields are ignored.
            _ => {}
        }
        if self.pending_bytes > self.max_event_bytes {
            self.clear();
            return Err(SseLimitExceeded::Event {
                limit: self.max_event_bytes,
            });
        }
        Ok(None)
    }

    fn clear(&mut self) {
        self.event_type = String::new();
        self.data_lines = Vec::new();
        self.pending_bytes = 0;
    }

    /// The event type resets on every dispatch attempt. An event whose data
    /// lines are all empty is not emitted (consumers parse the payload as JSON).
    fn dispatch(&mut self) -> Option<SseEvent> {
        let event_type = std::mem::take(&mut self.event_type);
        let lines = std::mem::take(&mut self.data_lines);
        self.pending_bytes = 0;
        lines.iter().any(|line| !line.is_empty()).then(|| SseEvent {
            event_type,
            data: lines.join("\n"),
        })
    }

    fn pending_data_len(&self) -> usize {
        self.data_lines.iter().map(String::len).sum()
    }
}

/// Incremental SSE parser.
///
/// Feed it arbitrary byte slices via [`push`]; it buffers incomplete lines
/// internally and emits complete [`SseEvent`]s. A peer that exceeds the line
/// or event cap fails the parser permanently.
pub struct SseParser {
    splitter: LineSplitter,
    assembler: EventAssembler,
    failed: Option<SseLimitExceeded>,
    chunk_count: usize,
    bytes_received: usize,
}

impl SseParser {
    pub fn new() -> Self {
        Self::with_limits(MAX_SSE_LINE_BYTES, MAX_SSE_EVENT_BYTES)
    }

    pub fn with_limits(max_line_bytes: usize, max_event_bytes: usize) -> Self {
        Self {
            splitter: LineSplitter::with_limit(max_line_bytes),
            assembler: EventAssembler::new(max_event_bytes),
            failed: None,
            chunk_count: 0,
            bytes_received: 0,
        }
    }

    /// Feed a chunk of bytes into the parser.
    ///
    /// Returns zero or more complete events extracted from the buffered data.
    ///
    /// # Errors
    /// [`SseLimitExceeded`] when a line or event outgrows its cap; every later
    /// call returns the same error.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseLimitExceeded> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        self.chunk_count = self.chunk_count.saturating_add(1);
        self.bytes_received = self.bytes_received.saturating_add(bytes.len());
        let mut events = Vec::new();
        let mut event_error = None;
        let assembler = &mut self.assembler;
        let split = self.splitter.push(bytes, |line| {
            if event_error.is_some() {
                return;
            }
            // Invalid UTF-8 (a gateway bug) is decoded lossily rather than
            // dropping the whole line.
            match assembler.push_line(&String::from_utf8_lossy(line)) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(error) => event_error = Some(error),
            }
        });
        match event_error.map_or(split, Err) {
            Ok(()) => Ok(events),
            Err(error) => {
                self.failed = Some(error);
                self.assembler.clear();
                Err(error)
            }
        }
    }

    /// Content-free parser counters safe for operational logging.
    pub fn diagnostics(&self) -> SseDiagnostics {
        SseDiagnostics {
            chunk_count: self.chunk_count,
            bytes_received: self.bytes_received,
            buffered_bytes: self.splitter.buffered_len(),
            pending_event_type_len: self.assembler.event_type.len(),
            pending_data_len: self.assembler.pending_data_len(),
        }
    }

    /// Signal end-of-stream. Flushes a pending event even without a trailing
    /// blank line (lenient — some servers omit the final blank). An
    /// unterminated final line is a torn read and is discarded.
    pub fn finish(mut self) -> Vec<SseEvent> {
        self.assembler.dispatch().into_iter().collect()
    }
}

#[cfg(test)]
trait PushOk {
    fn push_ok(&mut self, bytes: &[u8]) -> Vec<SseEvent>;
}

#[cfg(test)]
impl PushOk for SseParser {
    fn push_ok(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.push(bytes).expect("limits not exceeded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_event() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"event: ping\ndata: {\"type\": \"ping\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "ping");
        assert_eq!(events[0].data, "{\"type\": \"ping\"}");
    }

    #[test]
    fn multi_data_lines_joined() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"event: test\r\ndata: hello\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.push_ok(b"event: te").is_empty());
        assert!(p.push_ok(b"st\nda").is_empty());
        assert!(p.push_ok(b"ta: hello\n").is_empty());
        let events = p.push_ok(b"\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn utf8_emdash_split_across_chunks() {
        // em-dash: U+2014 = bytes E2 80 94
        let mut p = SseParser::new();
        // First chunk: "data: hello " + first byte of em-dash (0xE2)
        let mut chunk1 = b"data: hello ".to_vec();
        chunk1.push(0xE2);
        assert!(p.push_ok(&chunk1).is_empty());
        // Second chunk: remaining bytes of em-dash (0x80, 0x94) + " world\n\n"
        let mut chunk2 = vec![0x80, 0x94];
        chunk2.extend_from_slice(b" world\n\n");
        let events = p.push_ok(&chunk2);
        let all_data: String = events.iter().map(|e| e.data.clone()).collect();
        assert!(
            all_data.contains('\u{2014}'),
            "em-dash must survive chunking: {all_data:?}"
        );
        assert_eq!(all_data, "hello \u{2014} world");
    }

    #[test]
    fn data_no_space_after_colon() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"data:hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn gateway_trailing_whitespace() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"data: {\"type\":\"ping\"}     \n\n");
        assert_eq!(events.len(), 1);
        // Trailing whitespace is kept (JSON parser handles it fine)
        assert!(events[0].data.starts_with("{\"type\":\"ping\"}"));
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.push_ok(b"data: partial\n").is_empty());
        let events = p.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "partial");
    }

    #[test]
    fn no_event_type_defaults_to_empty() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"data: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "");
    }

    #[test]
    fn crlf_split_between_cr_and_lf() {
        // A chunk boundary falling between \r and \n must not produce a blank
        // line that dispatches the event early.
        let mut p = SseParser::new();
        let mut events = p.push_ok(b"event: test\r\ndata: a\r");
        events.extend(p.push_ok(b"\ndata: b\r\n\r"));
        events.extend(p.push_ok(b"\n"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "a\nb");
    }

    #[test]
    fn event_type_resets_after_dataless_event() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"event: ping\n\ndata: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "");
    }

    #[test]
    fn all_empty_data_lines_are_not_dispatched() {
        let mut p = SseParser::new();
        assert!(p.push_ok(b"data:\ndata:\n\ndata\n\n").is_empty());
        let events = p.push_ok(b"data:\ndata: x\n\n");
        assert_eq!(events[0].data, "\nx");
    }

    #[test]
    fn finish_discards_torn_final_line() {
        let mut p = SseParser::new();
        assert!(p.push_ok(b"event: e\ndata: {\"te").is_empty());
        assert!(p.finish().is_empty());
    }

    #[test]
    fn finish_flushes_complete_data_line_with_cr_tail() {
        let mut p = SseParser::new();
        assert!(p.push_ok(b"data: a\r").is_empty());
        let events = p.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "a");
    }

    #[test]
    fn bare_cr_line_endings() {
        let mut p = SseParser::new();
        // Bare \r as line ending (WHATWG SSE spec requires this)
        let events = p.push_ok(b"event: test\rdata: hello\r\r");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn bare_cr_mixed_with_lf() {
        let mut p = SseParser::new();
        // Mix of bare \r and \n
        let events = p.push_ok(b"event: test\rdata: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn bare_cr_between_events() {
        // Simulates the exact corruption pattern: two events separated only by \r
        let mut p = SseParser::new();
        let events = p.push_ok(b"data: {\"text\":\"hello\"}\r\rdata: {\"text\":\"world\"}\r\r");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"text\":\"hello\"}");
        assert_eq!(events[1].data, "{\"text\":\"world\"}");
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseParser::new();
        let events = p.push_ok(b"data: one\n\ndata: two\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "one");
        assert_eq!(events[1].data, "two");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // ========================================================================
    // Strategies
    // ========================================================================

    /// Generate a valid SSE data payload (simulating JSON `content_block_delta`).
    /// Includes characters known to cause issues: backticks, em-dashes, quotes,
    /// braces, newlines-within-JSON-strings (escaped as \n in JSON).
    fn arb_sse_data() -> impl Strategy<Value = String> {
        prop_oneof![
            // Simple ASCII
            "[a-zA-Z0-9 _.!?,]{1,100}",
            // JSON-like with special chars
            ("[a-zA-Z0-9 ]{0,30}", "[a-zA-Z0-9 ]{0,30}").prop_map(|(a, b)| {
                format!("{{\"type\":\"text_delta\",\"text\":\"{a} \u{2014} {b}\"}}")
            }),
            // Backtick + em-dash combos (the exact pattern from the bug)
            "[a-zA-Z0-9 ]{0,20}".prop_map(|s| { format!("{{\"text\":\"` \u{2014}{s}`:{{}}\"}}") }),
            // Multi-byte unicode characters
            "[a-zA-Z0-9\u{00e9}\u{2014}\u{1f600} ]{1,60}",
        ]
    }

    /// Generate one or more SSE events as a raw byte stream.
    fn arb_sse_stream() -> impl Strategy<Value = (Vec<u8>, Vec<SseEvent>)> {
        proptest::collection::vec((proptest::option::of("[a-z_]{1,20}"), arb_sse_data()), 1..8)
            .prop_map(|event_specs| {
                let mut bytes = Vec::new();
                let mut expected = Vec::new();
                for (event_type, data) in event_specs {
                    if let Some(ref et) = event_type {
                        bytes.extend_from_slice(format!("event: {et}\n").as_bytes());
                    }
                    // SSE data lines: split on actual newlines in the data payload
                    for line in data.split('\n') {
                        bytes.extend_from_slice(format!("data: {line}\n").as_bytes());
                    }
                    bytes.extend_from_slice(b"\n"); // blank line terminates event
                    expected.push(SseEvent {
                        event_type: event_type.unwrap_or_default(),
                        data,
                    });
                }
                (bytes, expected)
            })
    }

    // ========================================================================
    // Property A — Chunk-boundary independence
    //
    // The same byte stream, split at different points, must produce
    // identical events.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn prop_chunk_boundary_independence(
            (bytes, expected) in arb_sse_stream(),
            seed in 0u64..1000,
        ) {
            // Reference: feed all at once
            let mut ref_parser = SseParser::new();
            let ref_events = ref_parser.push_ok(&bytes);

            // Split at arbitrary points (deterministic from seed)
            let split_points = {
                let mut pts = Vec::new();
                let mut s = seed;
                let len = bytes.len();
                if len > 0 {
                    for _ in 0..=(s % 8) {
                        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        #[allow(clippy::cast_possible_truncation)]
                        pts.push((s as usize) % len);
                    }
                }
                pts.sort_unstable();
                pts.dedup();
                pts
            };

            let mut chunked_parser = SseParser::new();
            let mut chunked_events = Vec::new();
            let mut prev = 0;
            for &sp in &split_points {
                if sp > prev {
                    chunked_events.extend(chunked_parser.push_ok(&bytes[prev..sp]));
                    prev = sp;
                }
            }
            chunked_events.extend(chunked_parser.push_ok(&bytes[prev..]));

            prop_assert_eq!(
                &ref_events, &chunked_events,
                "Chunk splitting changed output (split at {:?})",
                split_points,
            );

            // Also verify against expected
            prop_assert_eq!(
                &ref_events, &expected,
                "Parser output doesn't match expected events",
            );
        }
    }

    // ========================================================================
    // Property B — Multi-byte UTF-8 safety
    //
    // Splitting a stream mid-codepoint must not lose data.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn prop_utf8_split_safety(
            text in "[a-z\u{00e9}\u{2014}\u{1f600}]{1,40}",
        ) {
            let payload = format!("{{\"text\":\"{text}\"}}");
            let raw = format!("data: {payload}\n\n");
            let raw_bytes = raw.as_bytes().to_vec();

            // Split at every possible byte position
            for split_at in 0..raw_bytes.len() {
                let mut p = SseParser::new();
                let mut events: Vec<SseEvent> = Vec::new();
                events.extend(p.push_ok(&raw_bytes[..split_at]));
                events.extend(p.push_ok(&raw_bytes[split_at..]));

                prop_assert_eq!(
                    events.len(), 1,
                    "Expected 1 event, got {} (split at byte {})",
                    events.len(), split_at,
                );
                prop_assert_eq!(
                    &events[0].data, &payload,
                    "Data mismatch when split at byte {}",
                    split_at,
                );
            }
        }
    }

    // ========================================================================
    // Property C — Multi-line data: concatenation
    //
    // Multiple `data:` lines before a blank line must be joined with `\n`.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn prop_multi_data_lines_joined(
            lines in proptest::collection::vec("[a-zA-Z0-9 ]{1,40}", 1..5),
        ) {
            let expected_data = lines.join("\n");
            let mut raw = String::new();
            for line in &lines {
                use std::fmt::Write;
                let _ = writeln!(raw, "data: {line}");
            }
            raw.push('\n'); // blank line terminates

            let mut p = SseParser::new();
            let events = p.push_ok(raw.as_bytes());

            prop_assert_eq!(events.len(), 1);
            prop_assert_eq!(&events[0].data, &expected_data);
        }
    }

    // ========================================================================
    // Property D — CRLF equivalence
    //
    // Replacing \n with \r\n must produce identical events.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn prop_crlf_equivalence(
            (lf_bytes, expected) in arb_sse_stream(),
        ) {
            // Convert all \n to \r\n
            let mut crlf_bytes = Vec::new();
            for &b in &lf_bytes {
                if b == b'\n' {
                    crlf_bytes.push(b'\r');
                }
                crlf_bytes.push(b);
            }

            let mut lf_parser = SseParser::new();
            let lf_events = lf_parser.push_ok(&lf_bytes);

            let mut crlf_parser = SseParser::new();
            let crlf_events = crlf_parser.push_ok(&crlf_bytes);

            prop_assert_eq!(
                &lf_events, &crlf_events,
                "CRLF vs LF produced different events",
            );
            prop_assert_eq!(&lf_events, &expected);
        }
    }

    // ========================================================================
    // Property E-bis — Bare \r equivalence
    //
    // Replacing \n with bare \r must produce identical events.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn prop_bare_cr_equivalence(
            (lf_bytes, expected) in arb_sse_stream(),
        ) {
            // Convert all \n to bare \r
            let cr_bytes: Vec<u8> = lf_bytes.iter().map(|&b| if b == b'\n' { b'\r' } else { b }).collect();

            let mut lf_parser = SseParser::new();
            let lf_events = lf_parser.push_ok(&lf_bytes);

            let mut cr_parser = SseParser::new();
            let cr_events = cr_parser.push_ok(&cr_bytes);

            prop_assert_eq!(
                &lf_events, &cr_events,
                "bare CR vs LF produced different events",
            );
            prop_assert_eq!(&lf_events, &expected);
        }
    }

    #[test]
    fn diagnostics_never_retain_payload_content() {
        let secret = "Bearer SECRET_SENTINEL prompt and tool schema";
        let wire = format!("event: error\ndata: {secret}");
        let mut parser = SseParser::new();
        parser.push_ok(wire.as_bytes());

        let diagnostics = parser.diagnostics();
        assert_eq!(diagnostics.chunk_count, 1);
        assert_eq!(diagnostics.bytes_received, wire.len());
        assert!(!format!("{diagnostics:?}").contains("SECRET_SENTINEL"));
    }

    // ========================================================================
    // Property E — Anthropic content_block_delta round-trip
    //
    // Simulates the real wire format and verifies the data: field survives
    // arbitrary chunking with full JSON fidelity.
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn prop_anthropic_delta_json_survives_chunking(
            // Text containing the exact characters from the bug report
            text in "[a-zA-Z0-9 `\u{2014}\"{}:]{1,80}",
        ) {
            // Escape for JSON string value
            let escaped = text
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            let json = format!(
                "{{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{escaped}\"}}}}"
            );
            let raw = format!("event: content_block_delta\ndata: {json}\n\n");
            let raw_bytes = raw.as_bytes().to_vec();

            // Feed as a single chunk (reference)
            let mut ref_parser = SseParser::new();
            let ref_events = ref_parser.push_ok(&raw_bytes);
            prop_assert_eq!(ref_events.len(), 1);

            // Verify the data parses as valid JSON
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&ref_events[0].data);
            prop_assert!(
                parsed.is_ok(),
                "Data field is not valid JSON: {:?} (data={:?})",
                parsed.err(),
                ref_events[0].data,
            );

            // Now split at every byte and verify identical output
            for split_at in 0..raw_bytes.len() {
                let mut p = SseParser::new();
                let mut events = p.push_ok(&raw_bytes[..split_at]);
                events.extend(p.push_ok(&raw_bytes[split_at..]));

                prop_assert_eq!(
                    events.len(), 1,
                    "Wrong event count (split at byte {})",
                    split_at,
                );
                prop_assert_eq!(
                    &events[0].data, &ref_events[0].data,
                    "Data mismatch at split byte {}",
                    split_at,
                );
            }
        }
    }
}

/// Model-based properties: a random *script* of logical SSE lines is rendered
/// to bytes with mixed `\n` / `\r\n` / `\r` terminators, and the expected
/// events are computed from the script itself (never by re-parsing bytes).
#[cfg(test)]
mod model_proptests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Line {
        Event(String),
        Data(String),
        Comment(String),
        Unknown(String),
        Blank,
    }

    #[derive(Debug, Clone, Copy)]
    enum Term {
        Lf,
        CrLf,
        Cr,
    }

    impl Term {
        fn bytes(self) -> &'static [u8] {
            match self {
                Term::Lf => b"\n",
                Term::CrLf => b"\r\n",
                Term::Cr => b"\r",
            }
        }
    }

    /// Field values never contain line terminators and never start with a
    /// space (the single optional space after the colon is rendered separately).
    fn arb_value() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(String::new()),
            "[a-zA-Z0-9_.!?,{}\":]{1,20}",
            "[a-z\u{00e9}\u{2014}\u{1f600}][a-z \u{00e9}\u{2014}\u{1f600}]{0,10}",
        ]
    }

    fn arb_line() -> impl Strategy<Value = (Line, bool, Term)> {
        let line = prop_oneof![
            3 => arb_value().prop_map(Line::Data),
            2 => arb_value().prop_map(Line::Event),
            1 => arb_value().prop_map(Line::Comment),
            1 => arb_value().prop_map(Line::Unknown),
            3 => Just(Line::Blank),
        ];
        let term = prop_oneof![Just(Term::Lf), Just(Term::CrLf), Just(Term::Cr)];
        (line, any::<bool>(), term)
    }

    /// Render the script; returns the wire bytes and the expected events.
    fn render(script: &[(Line, bool, Term)]) -> (Vec<u8>, Vec<SseEvent>) {
        let mut bytes = Vec::new();
        let mut expected = Vec::new();
        let mut event_type = String::new();
        let mut data: Vec<String> = Vec::new();
        let mut prev_cr = false;
        for (line, space, term) in script {
            let sp = if *space { " " } else { "" };
            let (text, is_blank) = match line {
                Line::Event(v) => (format!("event:{sp}{v}"), false),
                Line::Data(v) => (format!("data:{sp}{v}"), false),
                Line::Comment(v) => (format!(":{v}"), false),
                Line::Unknown(v) => (format!("x-unknown:{sp}{v}"), false),
                Line::Blank => (String::new(), true),
            };
            // `\r` directly followed by an empty line's `\n` is, on the wire,
            // one `\r\n` — unrepresentable as two lines, so avoid generating it.
            let term = if is_blank && prev_cr && matches!(term, Term::Lf) {
                Term::CrLf
            } else {
                *term
            };
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(term.bytes());
            prev_cr = matches!(term, Term::Cr);
            match line {
                Line::Event(v) => event_type.clone_from(v),
                Line::Data(v) => data.push(v.clone()),
                Line::Blank => {
                    if data.iter().any(|d| !d.is_empty()) {
                        expected.push(SseEvent {
                            event_type: event_type.clone(),
                            data: data.join("\n"),
                        });
                    }
                    event_type.clear();
                    data.clear();
                }
                Line::Comment(_) | Line::Unknown(_) => {}
            }
        }
        (bytes, expected)
    }

    fn feed(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        chunks.iter().flat_map(|c| p.push_ok(c)).collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// Whole-stream parse equals the script's logical events.
        #[test]
        fn prop_matches_model(script in proptest::collection::vec(arb_line(), 0..24)) {
            let (bytes, expected) = render(&script);
            prop_assert_eq!(feed(&[&bytes]), expected);
        }

        /// Any chunking — including a split inside `\r\n` or a codepoint —
        /// yields the same events as the model.
        #[test]
        fn prop_chunking_is_unobservable(
            script in proptest::collection::vec(arb_line(), 0..24),
            cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..12),
        ) {
            let (bytes, expected) = render(&script);
            let mut points: Vec<usize> = cuts
                .iter()
                .map(|i| if bytes.is_empty() { 0 } else { i.index(bytes.len() + 1) })
                .collect();
            points.sort_unstable();
            let mut chunks: Vec<&[u8]> = Vec::new();
            let mut prev = 0;
            for p in points {
                chunks.push(&bytes[prev..p]);
                prev = p;
            }
            chunks.push(&bytes[prev..]);
            prop_assert_eq!(feed(&chunks), expected);
        }

        /// Worst-case chunking: one byte at a time.
        #[test]
        fn prop_byte_at_a_time(script in proptest::collection::vec(arb_line(), 0..24)) {
            let (bytes, expected) = render(&script);
            let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
            prop_assert_eq!(feed(&chunks), expected);
        }
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn oversized_line_fails_loudly_and_stays_failed() {
        let mut p = SseParser::with_limits(8, 1024);
        let error = SseLimitExceeded::Line { limit: 8 };
        assert_eq!(p.push(b"data: 0123456789"), Err(error));
        assert_eq!(p.push(b"\n\n"), Err(error));
        assert_eq!(p.diagnostics().buffered_bytes, 0);
    }

    #[test]
    fn many_small_lines_hit_the_event_cap() {
        let mut p = SseParser::with_limits(64, 10);
        assert_eq!(p.push(b"data: abcd\ndata: efgh\n"), Ok(vec![]));
        assert_eq!(
            p.push(b"data: ijk\n"),
            Err(SseLimitExceeded::Event { limit: 10 })
        );
        assert_eq!(p.diagnostics().pending_data_len, 0);
    }

    #[test]
    fn event_cap_counts_type_and_data_and_allows_exact_fit() {
        let mut p = SseParser::with_limits(64, 4);
        let events = p.push(b"event: e\ndata: abc\n\n").unwrap();
        assert_eq!(events.len(), 1);
        let mut p = SseParser::with_limits(64, 4);
        assert_eq!(
            p.push(b"event: e\ndata: abcd\n\n"),
            Err(SseLimitExceeded::Event { limit: 4 })
        );
    }

    proptest! {
        /// For any caps and chunking, buffered bytes never exceed the line cap
        /// and pending event bytes never exceed the event cap.
        #[test]
        fn memory_stays_bounded(
            bytes in proptest::collection::vec(
                prop_oneof![Just(b'a'), Just(b':'), Just(b' '), Just(b'\n'), Just(b'\r')],
                0..200,
            ),
            line_cap in 1usize..20,
            event_cap in 1usize..30,
            cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..8),
        ) {
            let mut points: Vec<usize> =
                cuts.iter().map(|i| i.index(bytes.len() + 1)).collect();
            points.sort_unstable();
            let mut p = SseParser::with_limits(line_cap, event_cap);
            let mut prev = 0;
            for end in points.into_iter().chain([bytes.len()]) {
                let result = p.push(&bytes[prev..end]);
                let d = p.diagnostics();
                prop_assert!(d.buffered_bytes <= line_cap);
                prop_assert!(d.pending_data_len + d.pending_event_type_len <= event_cap);
                if result.is_err() {
                    prop_assert!(p.push(b"x").is_err());
                    break;
                }
                prev = end;
            }
        }
    }
}
