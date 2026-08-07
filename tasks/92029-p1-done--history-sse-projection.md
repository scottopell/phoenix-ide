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
- `tasks/92029-p1-done--history-sse-projection.md`

**Settled facts encoded**
- `ProductConversationRouteResolvesLegacyMemberLinks` now resolves legacy member links by deriving `root_row` as the sole transcript row in the same `ProductConversation` with no predecessor and `latest_row` as the sole row with `continued_in_conv_id = absent`, then emits `ProductConversationRouteResolved(route_target: root_row, anchor_row: target_row, live_target: latest_row)`.
- This removes the incorrect earlier assumption that a legacy member target was itself the product root, preserving explicit member anchors while keeping the stable product route keyed by the durable root and the default live target keyed by the latest continuation row.
- `ProductConversationTranscriptProjectsChronologically` now describes boundary projection from durable predecessor facts (`continuation_boundary_summary`) plus the `continued_in_conv_id` continuation edge instead of the stale `transcript_boundaries` helper reference.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; zero `severity: "error"` diagnostics.
- Command: `allium check specs/bedrock/bedrock.allium specs/sse_wire/sse_wire.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; zero `severity: "error"` diagnostics across both files.

**Review / evidence ledger**
- Self-review: replaced the legacy-member route rule’s implicit “target row is root/live row” behavior with explicit derived root/latest bindings so ordinary root routes can still default to latest while legacy member links preserve the exact anchor row.
- Self-review: replaced the stale `transcript_boundaries` guidance reference with the actual durable predecessor-boundary plus continuation-edge model already established elsewhere in bedrock.
- Reviewer follow-up addressed: removed the invalid first requirement that asserted the requested legacy member target had no predecessor.

**Speculation avoided**
- No additional files, helper fields, route types, or projection contracts were introduced; the update stays inside `specs/bedrock/bedrock.allium` and the task ledger.
- The route and transcript semantics remain derived from existing durable facts (`product_conversation`, predecessor absence, `continued_in_conv_id`, `continuation_boundary_summary`) rather than new stored projection authority.

**Commit**
- `45ff6fbaeb5fb07170256667e5d37175e066fe3c`
