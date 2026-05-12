Apply server-delivered pending events through the reducer on every init so the UI resumes mid-turn views on reconnect.

Phase 3 of the SSE ReplayRing rollout. Depends on task 62001 (Phase 2 wire format).

Scope:
- Update `transformInitData` in `ui/src/hooks/useConnection.ts` to extract the new init fields (`pending_anchor_sequence_id`, `pending_events`, `pending_truncated`) into the `InitPayload`.
- Update `SseInitFreshConnect` and `SseInitReconnectMerge` reducer rules in `ui/src/conversation/atom.ts` to match specs/conversation_atom/conversation_atom.allium:
    1. Apply DB snapshot (existing behaviour).
    2. Apply pending events through the per-event rules in seq order (SseTokenAccumulated, SseStateChangeApplied, SseMessageAppended/SseMessageDedupReplay, etc.). The existing `applyIfNewer` guard naturally drops anything already-seen.
    3. Final `lastSequenceId = max(currentLastSequenceId, payload.last_sequence_id)` as a safety belt.
- Add reducer tests in `ui/src/conversation/atom.test.ts`:
    Fresh connect with pending tokens rebuilds streamingBuffer.
    Reconnect with pending state_change restores in-flight phase.
    Reconnect with pending eager Message renders ToolUseBlock for in-flight tool round.
    pending_truncated=true with empty pending_events results in DB-only render (no partial replay).
    Replayed events with seq <= atom.lastSequenceId are dropped (existing dedup).
- Manual verification: disconnect mid-LLM-stream, reconnect, confirm streaming text continues. Disconnect mid-tool-execution, reconnect, confirm ToolUseBlock still rendered.
- Flip relevant 🚧 status rows in `specs/sse_wire/executive.md` to ✅ once landed.

Acceptance:
- `./dev.py check` clean.
- Manual reconnect-mid-turn shows no blank-UI window (was the original symptom this whole multi-phase work targets).
- pending_truncated path quietly does a DB-only render (no banner — that was the Q3 design decision).

Out of scope: integration tests at the runtime level (covered in Phase 2 task as the parity / end-to-end tests; UI-side coverage is in this task).
