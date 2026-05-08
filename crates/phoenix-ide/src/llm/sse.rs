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
//! 4. **`\r\n` and `\n`** — both line endings work.
//! 5. **Trailing whitespace tolerance** — the gateway may pad `data:` lines.

/// A fully assembled SSE event ready for dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or empty if none was sent.
    pub event_type: String,
    /// The `data:` field (multiple `data:` lines joined with `\n`).
    pub data: String,
}

/// Incremental SSE parser.
///
/// Feed it arbitrary byte slices via [`push`]; it buffers incomplete lines
/// internally and emits complete [`SseEvent`]s.
pub struct SseParser {
    /// Bytes received but not yet consumed as a full line.
    buf: Vec<u8>,
    /// Accumulated `event:` value for the current event.
    current_event: String,
    /// Accumulated `data:` lines for the current event (joined with `\n`).
    current_data: String,
    /// Rolling log of raw byte chunks received (kept for diagnostics on parse errors).
    /// Capped to avoid unbounded growth.
    raw_chunks_log: Vec<Vec<u8>>,
    /// Total bytes logged so far (to enforce cap).
    raw_bytes_logged: usize,
}

impl SseParser {
    /// Max bytes to keep in the raw chunk log (64 KB).
    const RAW_LOG_CAP: usize = 64 * 1024;

    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            current_event: String::new(),
            current_data: String::new(),
            raw_chunks_log: Vec::new(),
            raw_bytes_logged: 0,
        }
    }

    /// Feed a chunk of bytes into the parser.
    ///
    /// Returns zero or more complete events extracted from the buffered data.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        // Log raw chunks for diagnostics (capped to avoid unbounded growth)
        if self.raw_bytes_logged < Self::RAW_LOG_CAP {
            self.raw_chunks_log.push(bytes.to_vec());
            self.raw_bytes_logged += bytes.len();
        }
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();

        while let Some(eol_pos) = self.buf.iter().position(|&b| b == b'\n' || b == b'\r') {
            // Determine how many bytes the line ending consumes.
            let eol_len = if self.buf[eol_pos] == b'\r' {
                // \r\n counts as one line ending; bare \r also counts.
                if self.buf.get(eol_pos + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                }
            } else {
                1 // bare \n
            };

            // Extract the line *as bytes* (excluding the line ending).
            let line_bytes = &self.buf[..eol_pos];

            // Decode UTF-8. If the slice is invalid (should not happen with
            // well-formed SSE, but guard against gateway bugs), use lossy
            // conversion so we never silently drop an entire line.
            let line = String::from_utf8_lossy(line_bytes).into_owned();

            // Drain consumed bytes (line content + line ending).
            self.buf.drain(..eol_pos + eol_len);

            if line.is_empty() {
                // Blank line → dispatch accumulated event (if any data present).
                if !self.current_data.is_empty() {
                    events.push(SseEvent {
                        event_type: std::mem::take(&mut self.current_event),
                        data: std::mem::take(&mut self.current_data),
                    });
                }
            } else if let Some(data) = line.strip_prefix("data: ") {
                // SSE spec: multiple data lines are joined with \n.
                if self.current_data.is_empty() {
                    self.current_data = data.to_string();
                } else {
                    self.current_data.push('\n');
                    self.current_data.push_str(data);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                // `data:` with no space — also valid per SSE spec.
                if self.current_data.is_empty() {
                    self.current_data = data.to_string();
                } else {
                    self.current_data.push('\n');
                    self.current_data.push_str(data);
                }
            } else if let Some(event) = line.strip_prefix("event: ") {
                self.current_event = event.to_string();
            } else if let Some(event) = line.strip_prefix("event:") {
                self.current_event = event.to_string();
            }
            // Lines starting with `:` are comments — ignored per SSE spec.
            // Unknown prefixes are also ignored.
        }

        events
    }

    /// Return diagnostic info about the raw chunks received.
    /// Useful when a downstream JSON parse fails — helps distinguish
    /// "our parser lost bytes" from "the upstream sent garbage".
    pub fn diagnostic_dump(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "SseParser diagnostics: {} chunks, {} bytes logged",
            self.raw_chunks_log.len(),
            self.raw_bytes_logged,
        );
        // Show the last few chunks (most relevant to the error)
        let start = self.raw_chunks_log.len().saturating_sub(5);
        for (i, chunk) in self.raw_chunks_log[start..].iter().enumerate() {
            let display = String::from_utf8_lossy(chunk);
            let truncated = if display.len() > 500 {
                format!(
                    "{}...[truncated, {} bytes total]",
                    display.get(..500).unwrap_or(&display),
                    chunk.len()
                )
            } else {
                display.into_owned()
            };
            let _ = writeln!(
                out,
                "  chunk[{}]: ({} bytes) {:?}",
                start + i,
                chunk.len(),
                truncated,
            );
        }
        if !self.buf.is_empty() {
            let remaining = String::from_utf8_lossy(&self.buf);
            let _ = writeln!(
                out,
                "  remaining buf: ({} bytes) {:?}",
                self.buf.len(),
                remaining
            );
        }
        out
    }

    /// Signal end-of-stream. Flushes any pending event even without a trailing
    /// blank line (lenient — some servers omit the final blank).
    pub fn finish(mut self) -> Vec<SseEvent> {
        // If there's an unterminated event, emit it.
        let mut events = Vec::new();
        if !self.current_data.is_empty() {
            events.push(SseEvent {
                event_type: std::mem::take(&mut self.current_event),
                data: std::mem::take(&mut self.current_data),
            });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_event() {
        let mut p = SseParser::new();
        let events = p.push(b"event: ping\ndata: {\"type\": \"ping\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "ping");
        assert_eq!(events[0].data, "{\"type\": \"ping\"}");
    }

    #[test]
    fn multi_data_lines_joined() {
        let mut p = SseParser::new();
        let events = p.push(b"data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        let events = p.push(b"event: test\r\ndata: hello\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.push(b"event: te").is_empty());
        assert!(p.push(b"st\nda").is_empty());
        assert!(p.push(b"ta: hello\n").is_empty());
        let events = p.push(b"\n");
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
        assert!(p.push(&chunk1).is_empty());
        // Second chunk: remaining bytes of em-dash (0x80, 0x94) + " world\n\n"
        let mut chunk2 = vec![0x80, 0x94];
        chunk2.extend_from_slice(b" world\n\n");
        let events = p.push(&chunk2);
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
        let events = p.push(b"data:hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn gateway_trailing_whitespace() {
        let mut p = SseParser::new();
        let events = p.push(b"data: {\"type\":\"ping\"}     \n\n");
        assert_eq!(events.len(), 1);
        // Trailing whitespace is kept (JSON parser handles it fine)
        assert!(events[0].data.starts_with("{\"type\":\"ping\"}"));
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: partial\n").is_empty());
        let events = p.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "partial");
    }

    #[test]
    fn no_event_type_defaults_to_empty() {
        let mut p = SseParser::new();
        let events = p.push(b"data: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "");
    }

    #[test]
    fn bare_cr_line_endings() {
        let mut p = SseParser::new();
        // Bare \r as line ending (WHATWG SSE spec requires this)
        let events = p.push(b"event: test\rdata: hello\r\r");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn bare_cr_mixed_with_lf() {
        let mut p = SseParser::new();
        // Mix of bare \r and \n
        let events = p.push(b"event: test\rdata: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn bare_cr_between_events() {
        // Simulates the exact corruption pattern: two events separated only by \r
        let mut p = SseParser::new();
        let events = p.push(b"data: {\"text\":\"hello\"}\r\rdata: {\"text\":\"world\"}\r\r");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"text\":\"hello\"}");
        assert_eq!(events[1].data, "{\"text\":\"world\"}");
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseParser::new();
        let events = p.push(b"data: one\n\ndata: two\n\n");
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
            let ref_events = ref_parser.push(&bytes);

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
                    chunked_events.extend(chunked_parser.push(&bytes[prev..sp]));
                    prev = sp;
                }
            }
            chunked_events.extend(chunked_parser.push(&bytes[prev..]));

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
                events.extend(p.push(&raw_bytes[..split_at]));
                events.extend(p.push(&raw_bytes[split_at..]));

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
            let events = p.push(raw.as_bytes());

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
            let lf_events = lf_parser.push(&lf_bytes);

            let mut crlf_parser = SseParser::new();
            let crlf_events = crlf_parser.push(&crlf_bytes);

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
            let lf_events = lf_parser.push(&lf_bytes);

            let mut cr_parser = SseParser::new();
            let cr_events = cr_parser.push(&cr_bytes);

            prop_assert_eq!(
                &lf_events, &cr_events,
                "bare CR vs LF produced different events",
            );
            prop_assert_eq!(&lf_events, &expected);
        }
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
            let ref_events = ref_parser.push(&raw_bytes);
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
                let mut events = p.push(&raw_bytes[..split_at]);
                events.extend(p.push(&raw_bytes[split_at..]));

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
