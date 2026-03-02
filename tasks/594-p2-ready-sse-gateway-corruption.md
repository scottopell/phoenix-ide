---
created: 2026-03-02
priority: p2
status: ready
---

# SSE stream corruption from exe.dev LLM gateway

## Summary

The exe.dev LLM gateway (`http://169.254.169.254/gateway/llm`) intermittently
corrupts SSE event streams during long Anthropic API responses. This causes
JSON parse failures in Phoenix and aborted LLM turns.

## Symptoms

Two SSE events are smashed together mid-JSON, producing unparseable data:

```
Failed to parse SSE data: expected `,` or `}` at line 1 column 106 -
data: {"type":"content_block_delta",...,"partial_json":"ies\\n\\n- `
{"type":"con:"coevent: content_block_delta
{"type":"content_block_delta",...}
```

Pattern: occurs after ~500+ SSE chunks in a single stream. Not every long
stream is affected — intermittent, maybe 1 in 5 long conversations.

## Root Cause Analysis — CONFIRMED via pcap

The corruption occurs inside the exe.dev LLM gateway before the bytes
reach Phoenix. This is **proven at the TCP level** via packet capture.

### TCP-level evidence (2026-03-02)

Capture file: `sse-corruption-evidence.pcap` (49MB)

Stream to port 42182, Anthropic SSE for claude-sonnet-4-6:

**Clean event** (seq 2235, early in stream):
```
Chunk size: 0x91 (145 bytes)
data: {..."partial_json":"\":.\"Fou"}       }\n\n
                                   ^^ proper JSON close + event separator
```

**Corrupted event** (seq 22224, ~35s into stream, ~139th chunk):
```
Chunk size: 0x91 (145 bytes)  ← SAME size as clean events
data: {..."partial_json":"mo         }\n\neve} }\n
                                ^^^^^^^^^ spaces where closing "}}
                                          should be, "eve" fragment
                                          of next event leaked in
```

Key observations:
- Chunked TE frame sizes are correct (0x91 both times)
- The **content inside the chunk** is already garbled
- This rules out chunked TE reassembly — corruption happens upstream
  of the TE framing, in the gateway's response buffer
- Multiple events in the same stream showed corruption (missing opening
  quotes on `partial_json` values, garbled closing braces) before the
  fatal parse error
- Zero TCP retransmissions or resets in the capture
- The Shelley Go agent also uses this gateway and may hit the same bug

### Additional evidence

- Phoenix's `SseParser` (src/llm/sse.rs) has property tests proving
  chunk-boundary independence and correct multi-line data joining
- No `h2` crate in deps — purely HTTP/1.1 to gateway
- `SseParser::diagnostic_dump()` confirmed: corruption arrives in
  a single chunk from the gateway, not introduced by chunk reassembly

### Likely cause

The gateway is re-buffering upstream Anthropic SSE events before
re-chunking to the client. A race condition or buffer overwrite in
that layer is corrupting event content while preserving frame sizes.

## Diagnostic Capability

Phoenix has diagnostic infrastructure for capturing this:

- `SseParser::diagnostic_dump()` logs raw chunks (capped at 64KB) on parse failure
- Both `anthropic.rs` and `openai.rs` call `diagnostic_dump()` at ERROR level
  on SSE parse failures
- Capture command: `sudo tcpdump -i any -w /tmp/sse.pcap -s 0 'host 169.254.169.254'`

## What Phoenix Can Do

This is an upstream bug, but Phoenix could mitigate:

- [ ] Retry the LLM request on SSE parse failure (currently aborts the turn)
- [ ] Add a gateway health metric tracking corruption rate
- [ ] Consider buffering and validating complete SSE events before parsing JSON
- [ ] Report the bug to exe.dev with pcap evidence

## Gateway URL Conventions (reference)

The exe.dev gateway supports two equivalent path conventions:
- `{gateway}/{provider}/v1/...` — used by Phoenix
- `{gateway}/_/gateway/{provider}/v1/...` — used by Shelley Go agent

Documented in `src/llm.rs` module-level docs.

## Acceptance Criteria

- [ ] Intermittent SSE corruption is resolved (upstream fix or client-side retry)
- [ ] Long streaming conversations complete reliably
