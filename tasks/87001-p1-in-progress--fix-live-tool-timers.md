# Fix live tool timers that keep ticking after tool results arrive

## Problem

Inline tool cards show a live elapsed timer from `agent.display_data.tool_starts[tool_use_id]`. In current live conversations, the timer can keep ticking even after the tool output is visible, until the entire LLM/tool turn finishes.

This violates the existing render-unit/tool ownership contract in `specs/messagelist-render-units/requirements.md`:

- every tool result must be paired with the tool call that produced it
- interleaved system/status messages must not make a result appear orphaned or leave the originating tool card in an in-flight state

The screenshot shows this exact failure: tool output is rendered under `read_file`, but the header still shows a ticking `• 11s` elapsed indicator.

## Screenshot evidence to preserve in tests

The attached screenshot showed the following self-contained failure state:

- Multiple completed-looking tool cards are visible in an agent turn.
- Two `bash` tool cards near the top still show live elapsed headers like `bash • 24s`.
- Below that, a `read_file` card shows `read_file • 11s` in the header while also rendering completed output:
  - the input/path row is visible (`/Users/scottopell/dev/...`)
  - a rendered file preview is visible with numbered lines (`1 # spEARS`, `3 spEARS (Simple Project ...`, `+35 more lines`)
  - the left edge has the green success/accent bar used for completed output
- Additional `read_file • 11s` cards below show completed path/range text such as `SPEARS.md:1–240` while the header timer remains live.
- The important observable is: **tool output is already visible and appears complete, but the header still renders the live `.tool-block-elapsed` timer instead of freezing/removing it and showing static completion duration.**

The regression test should encode these facts directly; it must not depend on access to the original screenshot.

## Initial investigation

Relevant code paths:

- `ui/src/components/MessageComponents.tsx`
  - `ToolUseBlockImpl` hides the live timer only when `result != null`.
  - Static duration is read from `result.display_data.duration_ms`.
- `ui/src/conversation/renderUnits.ts`
  - `buildRenderUnits` attaches `tool` messages to the active `agent_turn` via `toolResultsByUseId`.
  - Existing tests cover trailing tools and system-message interleaving, but not the observed "output visible while header still thinks in-flight" state.
- `ui/src/conversation/atom.ts`
  - `sse_message_updated` merges `durationMs` into `display_data.duration_ms` for the tool-result message.
- `crates/phoenix-ide/src/runtime/executor.rs`
  - Persisted tool rows already merge `duration_ms` into display data and emit `MessageUpdated` after broadcasting each tool message.

Likely failure modes to verify:

1. The live UI receives a tool-result update/content path that makes output render before the `result` prop becomes non-null.
2. A `message_updated` duration event is applied to the tool result, but the owning `AgentMessage`/`ToolUseBlock` does not re-render with the updated `toolResultsByUseId` entry.
3. A duplicate persisted assistant/tool `message` is sequence-dropped or deduped in a way that prevents the live `tool` message from reaching `buildRenderUnits` until a later turn-finalizing event.
4. Compact/full tool rendering paths disagree about what counts as completed.

## Plan

1. Add a failing regression test for the live state shown in the screenshot:
   - agent message has a `tool_use` and `display_data.tool_starts`
   - live state includes visible/updated tool result content or duration before the turn fully returns to idle
   - assert the tool output is visible and `.tool-block-elapsed` is absent immediately
   - assert the static duration is shown when `duration_ms` is known
2. Trace the actual failing boundary with the test:
   - if `buildRenderUnits` has the tool message, fix `ToolUseBlock`/memoization completion detection
   - if `buildRenderUnits` does not have the tool message, fix SSE reducer/dedup/sequence handling so live tool messages attach as soon as they arrive
   - if only `message_updated` arrives before the tool message, do not fake completion from the parent `tool_starts`; ensure the real tool message/content is delivered and paired, or add a typed pending-complete representation if that is the intended event ordering
3. Keep the representation single-source:
   - completed state should be derived from the paired tool-result message, not from duplicated state in `tool_starts`
   - `duration_ms` remains on the tool-result message display data
4. Run targeted tests:
   - `pnpm --dir ui test MessageComponents.test.tsx renderUnits.test.ts atom.test.ts` (or the repo's equivalent through `./dev.py` if needed)
   - run broader `./dev.py check` if the patch touches Rust/SSE types

## Acceptance criteria

- Tool card elapsed timers stop as soon as the corresponding tool result is available in the live transcript.
- Static duration displays from `duration_ms` when present.
- Interleaved system/status messages do not keep tool cards in an in-flight visual state.
- Regression coverage fails on the current broken behavior and passes with the fix.
- No parallel completed-state representation is introduced.
