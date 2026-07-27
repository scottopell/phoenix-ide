Child of task 92018.

# Seamless History / SSE projection across bedrock + sse_wire

## Objective
Specify the seamless projection of lifecycle/history truth from bedrock into SSE/wire-facing spec artifacts so History appears as the same product conversation without reviving legacy lifecycle names or split authority.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/sse_wire/requirements.md`
- any existing `specs/sse_wire/*.allium` files if they already exist and are the minimal place to encode projection behavior
- read-only context from `specs/conversation-retrieval/requirements.md`
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- Product lifecycle is Open/History only; Close is the action, History the resulting state.
- Context continuation remains one product conversation across linked durable rows.
- Latest-row authority is derived from `continued_in_conv_id`; projections must not introduce a second authoritative “latest/current” model.
- History is not a separate product family; wire/history projection must remain seamless for the same conversation.
- This task owns projection behavior only; it must not absorb durable Close orchestration, WorkScope retirement classification, or approved-task placement behavior.

## Required work contract
- Keep scope to bedrock + sse_wire projection semantics only.
- Use only already-grounded projection concepts; do **not** add speculative event names, placeholder wire fields, guessed transport helpers, or invented cross-file contracts.
- Preserve artifact voice: Allium behavior in existing `.allium`, timeless normative rules in `requirements.md` only where needed.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/durable-workflows/**`
- `specs/projects/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete rules/requirements
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/sse_wire/sse_wire.allium`
- `tasks/92029-p1-done--history-sse-projection.md`

**Settled facts encoded**
- `specs/bedrock/bedrock.allium` no longer uses invalid `extend entity ProductConversation` syntax. Product-conversation root/latest identity remains derived from `transcript_rows` plus `continued_in_conv_id`, with no writable `transcript_root`, `latest_activity_row`, `route_target`, or `live_target` fields introduced.
- Bedrock keeps boundary authority singular on durable predecessor facts: `ContinuationHandoffBecomesBoundary` preserves `continuation_boundary_summary` on the handed-off predecessor only, while `ContinuationBoundarySummaryLivesOnHandedOffPredecessor` avoids any parallel `List<TranscriptBoundary>` authority.
- Bedrock still encodes one-root / one-latest topology structurally via `ProductConversationHasSingleRootRow` and `ProductConversationHasSingleTopologyLatestRow`, so list ordering and live-target semantics stay derived from continuation topology rather than stored helper fields.
- The history transition rule no longer self-triggers by name: `ProductConversationTransitionsToHistory` now listens for `ProductConversationTransitionToHistoryRequested(product_conversation)`, removing the obvious naming conflict without broadening scope.
- `specs/sse_wire/sse_wire.allium` remains the place where projection payloads may carry root/latest identifiers (`root_conversation_id`, row/member IDs, boundary predecessor/successor IDs), but those payloads are projections of durable facts rather than new writable authorities.
- SSE transcript/boundary projection continues to key everything by durable root identity: `PersistedTranscriptMessageProjectsIntoRootStream`, `ContinuationBoundaryBroadcast`, and the root-aggregate init requirements keep root/latest IDs as projection payload derived from persisted rows and continuation edges.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; parsed successfully with zero `severity: "error"` diagnostics after removing the invalid `extend entity ProductConversation` block.
- Command: `allium check specs/sse_wire/sse_wire.allium`
  - Result: exit `0`; zero `severity: "error"` diagnostics.
- Command: `allium check specs/bedrock/bedrock.allium specs/sse_wire/sse_wire.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; zero `severity: "error"` diagnostics across both files.

**Review / evidence ledger**
- Self-review: corrected reopened task 92029 by removing the invalid `extend entity ProductConversation` block instead of trying to preserve derived fields with unsupported syntax.
- Self-review: removed the parallel boundary authority introduced by `List<TranscriptBoundary>`/`append_boundary(...)`; the final bedrock spec leaves boundary truth on predecessor `continuation_boundary_summary` plus the successor continuation edge.
- Self-review: renamed the self-triggering `ProductConversationTransitionsToHistory` trigger to `ProductConversationTransitionToHistoryRequested(...)` to eliminate the obvious naming conflict without broadening the behavior surface.

**Speculation avoided**
- No requirements, executive docs, UI/code, dedicated chain route/page, helper storage fields, or duplicate persisted fields for root/latest/boundary authority were added.
- The final change uses existing durable facts (`transcript_rows`, `continued_in_conv_id`, `continuation_boundary_summary`) rather than inventing writable projection state.

**Commit**
- `adafbe81a8dffeafc471cf50487f3e6b7be45fd0`
