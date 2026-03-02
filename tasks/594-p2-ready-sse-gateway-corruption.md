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

## Root Cause Analysis

A 4-expert subagent panel reviewed client-side code and reached consensus:
the corruption is in the exe.dev gateway's chunked transfer-encoding
reassembly, not in Phoenix's SSE parser.

Evidence:
- Phoenix's `SseParser` (src/llm/sse.rs) has property tests proving
  chunk-boundary independence and correct multi-line data joining
- The corrupted output contains event type strings (`event: content_block_delta`)
  interleaved with data fields — this can only happen if the gateway drops
  the `\n\n` event separator between two SSE events
- No `h2` crate in deps — purely HTTP/1.1 to gateway, ruling out HTTP/2 framing
- TCP captures (pcap) during successful runs show zero retransmissions/resets
- The Shelley Go agent also uses this gateway (via `_/gateway/` path prefix)
  and may experience the same issue

## Diagnostic Capability

Phoenix already has diagnostic infrastructure for capturing this:

- `SseParser::diagnostic_dump()` logs raw chunks (capped at 64KB) on parse failure
- Both `anthropic.rs` and `openai.rs` call `diagnostic_dump()` at ERROR level
  on SSE parse failures
- To get definitive proof: `tcpdump -i any -w /tmp/sse.pcap -s 0 'host 169.254.169.254'`
  during a long stream, then inspect whether the TCP payload already contains
  the corruption or if Phoenix introduced it

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
