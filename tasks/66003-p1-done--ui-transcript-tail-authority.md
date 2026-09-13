# Make transcript unit projection and virtualized tail authoritative

## Priority

P1. Separate UI transcript-tail correctness from LLM/runtime liveness.

## Problem

A healthy idle conversation can contain a complete persisted transcript while the UI appears to end on older plain or queued tool calls. The owning UI path must preserve the terminal transcript unit, pair source tool results correctly, and physically expose the final unit at the virtualized tail.

Production conversation `586e7f0b-9a47-45d0-8f11-55990a2decd8` is immutable evidence only. Do not mutate, replay, restart, or derive live fixture data from it.

## Owning boundary

Bound implementation to:

1. transcript message → semantic render-unit projection;
2. `tool_use` → source-side `tool` result pairing by `tool_use_id`;
3. `MessageList` → `VirtualTranscript` physical tail placement and visibility;
4. the existing deterministic message-list fixture journey.

The fix belongs in the smallest demonstrated UI projection/presentation boundary. Do not change persistence, durable workflows, provider/LLM liveness, or tool execution.

## Required production-shaped regression

Use synthetic deterministic messages with this ordered shape:

1. an assistant `send_conversation_message` `tool_use`;
2. its matching source-side `tool` result whose payload contains `outcome: "queued_as_steering"`;
3. later terminal assistant text with a unique assertion marker.

The regression must prove:

- `buildHistoricalUnits` (or the owning projection) pairs the result with the source `tool_use` and orders the terminal assistant unit last;
- the source tool renders **completed/success**, not waiting, running, missing-result, or unresolved/queued source work;
- the terminal assistant text is mounted and intersects the transcript viewport at the physical tail;
- the assertion runs through the owning real `MessageList`/`VirtualTranscript` fixture journey, not a mocked virtualization passthrough;
- variable-height preceding content is sufficient to exercise measured virtualized tail placement at a production-relevant mobile viewport;
- no timer, retry, or liveness inference is introduced as correctness logic.

## Implementation scope

- Read and honor:
  - `specs/conversation_atom/conversation_atom.allium`
  - `specs/messagelist-render-units/requirements.md`
  - `specs/messagelist-render-units/render_units.allium`
  - `specs/virtual-transcript/requirements.md`
  - `specs/virtual-transcript/virtual_transcript.allium`
- Start at:
  - `ui/src/conversation/renderUnits.ts`
  - `ui/src/components/MessageList.tsx`
  - `ui/src/components/VirtualTranscript.tsx`
  - `ui/src/components/agentTurnToolStrip.ts`
  - `ui/src/components/MessageComponents.tsx`
  - `ui/src/fixtures/messageList/`
  - `ui/scripts/capture-message-list.mjs`
- Extend the existing message-list Ladle/QA surface, which already exercises real `MessageList` and real `VirtualTranscript`; do not create a parallel fixture harness.
- Update timeless specs/executive verification only if the demonstrated invariant is not already explicit.

## Acceptance criteria

- [ ] Create this task with `taskmd new --priority p1` and immediately transition it to `in-progress` with `taskmd status` after proposal approval.
- [ ] Production conversation `586e7f0b-9a47-45d0-8f11-55990a2decd8` remains read-only and unchanged.
- [ ] A pure projection test proves the matching `queued_as_steering` result is attached to the source tool unit and terminal assistant text is the last semantic unit.
- [ ] A presentation test proves the paired source tool is completed/success and contains none of: waiting, running, missing-result, or unresolved source work.
- [ ] A deterministic message-list fixture uses the required production-shaped sequence and real virtualization.
- [ ] Browser/fixture assertions prove the unique terminal assistant marker is mounted, visible, and at the transcript tail on mobile.
- [ ] Existing reader-owned scroll anchoring, prefix restoration, streaming tail, and jump-to-newest regressions remain green.
- [ ] Focused UI tests and `./dev.py qa message-list` pass.
- [ ] `./dev.py check` passes.
- [ ] Independent adversarial review attempts to falsify projection order, pairing, variable-height measurement, tail visibility, reconnect behavior, and viewport ownership; all blocking findings are resolved.
- [ ] Push as a separate PR, verify exact PR HEAD, wait for exact-head CI and Codex/reviewer result, and report an evidence-based merge/no-merge decision.
- [ ] Do not deploy.

## Explicit non-goals

- Persistence or schema changes.
- Runtime/durable workflow, provider, LLM retry, or tool-execution changes.
- Mutating or replaying the production evidence conversation.
- A generic scroll rewrite, unrelated mobile cleanup, or visual redesign.
