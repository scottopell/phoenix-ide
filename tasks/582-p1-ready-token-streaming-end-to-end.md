---
created: 2026-02-28
number: 582
priority: p1
status: ready
slug: token-streaming-end-to-end
title: "Token streaming end-to-end: LLM provider → SSE → UI display"
---

# Token Streaming End-to-End

## Context

Read first:
- `specs/bedrock/design.md` — "Token Streaming Architecture" section
- `specs/bedrock/requirements.md` — REQ-BED-025
- `specs/llm/design.md` — "Streaming Interface (REQ-LLM-009)" section
- `specs/api/design.md` — "Token Streaming Events" section
- `specs/ui/design.md` — "Token Streaming Display" section
- `specs/ui/requirements.md` — REQ-UI-019

This is the coordinated backend + UI task. Tokens flow:
`Provider HTTP stream → StreamToken effect → broadcast_tx → SSE token event → UI reducer → StreamingMessage component`

## What to Do

### Backend: LLM Streaming Interface

1. **Add `complete_streaming()` to `LlmService` trait** per LLM design spec:
   ```rust
   async fn complete_streaming(
       &self,
       request: &LlmRequest,
       chunk_tx: &broadcast::Sender<TokenChunk>,
   ) -> Result<LlmResponse, LlmError>
   ```
   Default implementation calls `complete()` (no streaming). Existing test fakes
   are unaffected.

2. **Implement streaming for Anthropic provider** (primary provider):
   - Use streaming SSE from Anthropic Messages API
   - `content_block_start { type: "text" }` → start forwarding text deltas
   - `content_block_delta { text_delta }` → send via `chunk_tx`
   - `content_block_start { type: "tool_use" }` → accumulate tool input JSON
   - `message_delta` → extract usage data
   - `message_stop` → assemble final `LlmResponse`

3. **Add `StreamToken` fire-and-forget effect** and `TokenChunk` type per bedrock spec.

4. **In executor**: when dispatching `RequestLlm`, create a `broadcast::channel` for
   tokens. Pass the sender to `complete_streaming()`. Forward received chunks as SSE
   `token` events via the conversation broadcaster.

### Backend: SSE Token Events

5. **Add `token` SSE event type** per API design spec:
   ```
   event: token
   data: {"text": "partial...", "request_id": "req_abc"}
   ```
   Include `request_id` so the UI can correlate and discard stale tokens.

6. Token events have no `sequence_id` — they are ephemeral, not persisted, not
   replayable on reconnect.

### UI: Streaming Display

7. **Add `sse_token` action** to the conversation reducer (should already be defined
   from task 581). Tokens accumulate in `streamingBuffer` on the atom.

8. **Create `StreamingMessage` component:**
   ```typescript
   function StreamingMessage({ buffer }: { buffer: StreamingBuffer | null }) {
     if (!buffer) return null;
     return <div className="streaming-message">{buffer.text}</div>;
   }
   ```
   Render below the message list when buffer is non-null.

9. **The atomic swap:** When `sse_message` fires, the reducer clears `streamingBuffer`
   and appends the finalized message in one call — one React render. The streaming text
   and final message cannot both be visible.

10. **Scroll behavior** (REQ-UI-019): During streaming, auto-scroll to bottom unless
    user has scrolled up. Provide "jump to live" affordance if scrolled away.

## Acceptance Criteria

- Anthropic responses stream token-by-token to the UI
- Other providers fall back to non-streaming (default implementation)
- Token events appear in SSE stream with `request_id`
- UI shows partial text growing during generation
- When response completes, streaming text is replaced by rendered markdown — no flicker,
  no duplication, no content loss
- Reconnecting mid-stream shows "thinking..." until new tokens arrive or final message
  arrives (missed tokens are acceptable)
- `./dev.py check` passes
- Manual test: send a message that generates a long response, verify token-by-token
  display

## Dependencies

- Task 577 (typed effect channels — provides the effect dispatch mechanism)
- Task 581 (conversation atom — provides the reducer and streaming buffer)

## Files Likely Involved

### Backend
- `src/llm/mod.rs` — LlmService trait, TokenChunk type
- `src/llm/anthropic/` — streaming implementation
- `src/state_machine/effect.rs` — StreamToken effect
- `src/runtime/executor.rs` — token broadcast wiring
- `src/api/sse.rs` — token SSE event type

### UI
- `ui/src/conversation/atom.ts` — sse_token action handler
- `ui/src/components/StreamingMessage.tsx` — NEW: streaming display
- `ui/src/components/MessageList.tsx` — render StreamingMessage
- `ui/src/hooks/useConnection.ts` — handle token SSE events
