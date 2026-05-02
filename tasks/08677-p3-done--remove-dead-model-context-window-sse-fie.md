---
created: 2026-04-21
priority: p3
status: done
artifact: src/runtime.rs
---

# Remove dead model_context_window from SSE init payload

## Summary

After task 08676 moved the frontend to derive the model context window
from `availableModels`, the `model_context_window` field on the SSE
init payload is unused. Remove it from the backend response and the
frontend type to stop shipping dead data on every connect.

## Context

Pre-08676, the frontend stored `atom.contextWindow.total` alongside
`used`, populated from this SSE field. Post-08676, `total` was dropped
in favor of deriving it at read-time from the ModelInfo registry
already on the client. The SSE field still ships but is read by
nothing.

Confirmed dead on the client:
- `grep model_context_window ui/src` returns only the optional field
  declaration at `ui/src/api.ts:177` -- no readers.

## Call sites to remove

Backend:
- `src/runtime.rs:163` -- `model_context_window: usize` on the `Init`
  variant of the runtime event enum.
- `src/api/handlers.rs:1211,1280,2724,2743` -- two places that build
  an Init event (initial subscribe + reconnect) each compute the value
  via `state.llm_registry.context_window(model_id)` and thread it into
  the event.
- `src/api/sse.rs:46,65` -- the sse serialization layer destructures
  the field out of the variant and writes it into the JSON payload.

Frontend:
- `ui/src/api.ts:177` -- optional `model_context_window?: number` on
  the SSE init data type.

## Acceptance Criteria

- [ ] `src/runtime.rs` `Init` variant no longer carries
      `model_context_window`.
- [ ] Both Init-building handlers in `src/api/handlers.rs` stop
      computing and passing the field.
- [ ] `src/api/sse.rs` no longer destructures or writes the field.
- [ ] `ui/src/api.ts` SSE init payload type drops
      `model_context_window`.
- [ ] `cargo check`, `cargo test`, `./dev.py check` all pass.
- [ ] `cd ui && npx tsc --noEmit` passes.
- [ ] Manual: open an existing conversation, verify the context bar
      still shows the correct denominator (derived from ModelInfo).

## Notes

- Low priority: the dead field is a few bytes on the wire, no
  correctness bug. File it so it doesn't get forgotten.
- `llm_registry.context_window()` stays -- still used by
  `/new` flows and the model registry API. Only the SSE path is dead.
