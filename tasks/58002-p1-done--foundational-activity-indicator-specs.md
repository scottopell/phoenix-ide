# Foundational specs: reliable agent activity indicators

## What this task covered

Drafting and stabilising the complete spec set for the "what is the
agent doing right now?" indicators in the conversation UI. Specs only.
Implementation lives in task 58001.

## Deliverables (all landed on PR #155)

- `specs/working-phase-visibility/`
  - `requirements.md` — 8 requirements (REQ-WPV-001..008)
  - `design.md` — wire/runtime/client changes
  - `executive.md` — status table + cross-spec dependencies
  - `working-phase-visibility.allium` — formal behavioural spec
- `specs/sse_wire/sse_wire.allium` — extended:
  - `llm_first_byte` added to `EphemeralEventAppendedToReplayRing`
    whitelist
  - Cross-spec "when adding a new SSE variant" checklist block
    embedded above the rule (7 places a new variant must land)

## Key decisions captured in the specs

- **Server-authoritative timestamps reuse the existing row field.**
  `Conversation.state_updated_at: DateTime<Utc>` (`db.rs:676`,
  bumped on every state transition) is exposed on Init via the
  existing `#[serde(flatten)]` on `EnrichedConversation`; same value
  added to `StateChange` as a typed field. RFC3339 on both carriers.
  Client converts to ms once at the SSE-handler boundary. No parallel
  representations.
- **Pending assistant bubble is a synthetic render unit**
  (`pending_agent` tail unit in `renderUnits.ts`), NOT a retained
  empty message — text-only LLM responses aren't persisted until
  after the `LlmResponse` transition completes
  (`state_machine/transition.rs` ~L711).
- **Per-tool started_at** on the assistant message's
  `display_data.tool_starts: BTreeMap<String, i64>` map keyed by
  `tool_use_id`. NOT a new field on `ContentBlock::ToolUse`
  (persisted, cross-provider; UI metadata doesn't belong there).
- **Retry is a modifier on the phase base reason**, not a precedence
  replacement. `executing bash 12s (retry 2/5)` retains the phase
  signal.
- **`TurnRetryContext` cleared on AgentDone / Error** (turn boundary),
  not per-phase exit, so the retry suffix survives intra-turn phase
  transitions (REQ-WPV-003).
- **Heartbeat watchdog at 35s threshold** (2.3x the 15s server
  keep-alive). Fed by switching the server keep-alive from SSE
  comment to a typed `ping` event with non-empty data payload (axum's
  `Event::data` drops empty-data events).
- **Explicit listener registration list.** Native EventSource has no
  wildcard for named events; the SSE client layer must register a
  listener per event type, each bumping `lastSseEventAt`.
- **Connection state never masks agent state.** Reconnect captures
  base_reason_text + retry context + frozen elapsed; rendered as
  `reconnecting (N) — last: thinking 12s (retry 2/5)`.
- **First-byte sub-phase split**: `thinking Ns` → `streaming` (no
  counter — the stream itself is the progress signal). Driven by
  new `LlmFirstByte` SSE event with its own monotonic sequence_id.

## Cross-spec dependency note

The spec inlines `value RetryContext` and the helpers
`render_retry_modifier_for(view)` / `render_frozen_retry_modifier(retry)`
as `PLACEHOLDER` blocks. The sibling spec (task 58003) replaces them
and adds the `use` import.

## Process artifacts produced alongside

- A **Helpers block** in `working-phase-visibility.allium` listing
  every helper used in rules with signatures and semantics — Allium
  has no formal function declarations, so centralising them makes
  drift visible.
- **Three orphan-cleanup rules** for `InflightToolTimer`
  (ConversationBecameTerminal, ToolPhaseExit, ConversationHardDeleted)
  closing the "timer outlives its conversation" state-machine hole.
- The **cross-spec authoring checklist** at the top of sse_wire's
  `EphemeralEventAppendedToReplayRing` rule (the authoritative
  source the next contributor will see).

## Review history

8 rounds of automated review (Copilot + chatgpt-codex-connector)
caught: wire-shape mismatches (`DateTime<Utc>` vs `i64`), Allium
grammar issues (no if/then/else expressions; no destructuring in
`when:`; entity-instance assignment not supported in `ensures:`),
undeclared helpers, cross-file drift between requirements/design/
executive/.allium, stale path:line citations, and several structural
bugs (singleton-bubble-not-reusable, missing transient connection
state coverage, missing Init-seed for reload, streaming-phase-exit
reset, retry context preservation in frozen snapshot, KeepAlive
empty-data trap, replay-ring whitelist gap).

Distilled lessons captured in task 58004 (spec authoring pre-flight
discipline).

## Files

- specs/working-phase-visibility/{requirements,design,executive}.md
- specs/working-phase-visibility/working-phase-visibility.allium
- specs/sse_wire/sse_wire.allium (extended)
