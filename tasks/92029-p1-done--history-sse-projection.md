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
- `tasks/92029-p1-in-progress--history-sse-projection.md`

**Settled facts encoded**
- `specs/bedrock/bedrock.allium` now projects one stable product-conversation route from the durable root via `ProductConversationRouteResolvesLegacyMemberLinks`; legacy member links resolve the root route plus an exact anchor row while default live targeting comes from `product_conversation.live_target`, derived from topology rather than a duplicate persisted latest field.
- Bedrock now models seamless transcript projection over the root aggregate: `ProductConversationTranscriptProjectsChronologically` requires root-to-latest chronological row projection, exact continuation boundaries between adjacent rows, and persisted messages as the sole transcript authority.
- Continuation joins now carry exactly one visible boundary sourced from the exact persisted continuation summary that initialized the successor: `ContinuationHandoffBecomesBoundary`, `ContinuationBoundaryMatchesExactPersistedSummary`, and `AtMostOneBoundaryPerContinuationJoin` forbid boundary/message duplication and keep handed-off predecessors inside the Open aggregate.
- History/Open projection semantics stay aggregate-owned: `ProductConversationHistoryProjectionIsReadOnly` makes History read-only/no-input, while `ProductConversationOpenProjectionKeepsHandedOffRowsInsideOpen` states that a handed-off predecessor inside an Open aggregate is a read-only segment, not History.
- Aggregate list/navigation semantics are singular-by-root: `ProductConversationListProjectsOncePerRoot` plus root/live-target invariants require one list projection per root, root-owned identity/title, and latest-activity ordering without inventing parallel lifecycle/latest/topology authority.
- `specs/sse_wire/sse_wire.allium` now defines root-keyed init/replay projection obligations via `RootConversationStreamOpened`: init/replay must serve the durable root aggregate, use persisted messages as transcript authority, project transcript rows chronologically, project continuation boundaries exactly once, and derive default live targeting from topology rather than client authority.
- SSE projection now models transcript rows and continuation boundaries as wire projections, not client-owned facts: `PersistedTranscriptMessageProjectsIntoRootStream`, `ContinuationBoundaryBroadcast`, and `RootConversationInitUsesDurableAggregateProjection` preserve member/message identity across reconnect/replay while keeping the client from inventing transcript state.
- Aggregate lifecycle projection is singular and atomic on the wire: `AggregateLifecycleProjectionBroadcast` states that Open/History comes from the product aggregate, handed-off predecessors do not project as History while the aggregate stays Open, and the whole aggregate lifecycle appears atomically from the UI perspective.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; no `severity: "error"` diagnostics.
- Command: `allium check specs/sse_wire/sse_wire.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; no `severity: "error"` diagnostics.
- Command: `allium check specs/bedrock/bedrock.allium specs/sse_wire/sse_wire.allium`
  - Result: exit `1` with existing `info`/`warning` diagnostics only; no `severity: "error"` diagnostics.

**Review / evidence ledger**
- Self-review: corrected initial malformed rule insertion in `specs/sse_wire/sse_wire.allium` so new projection rules are sibling rules rather than nested accidentally inside existing rules.
- Self-review: removed an unused speculative value type from `specs/sse_wire/sse_wire.allium`; the final change keeps only projection rules/entities needed for the requested behavioral contract.
- Self-review: confirmed the edited specs avoid dedicated chain-route/page behavior and instead keep projection keyed by the root conversation route.

**Speculation avoided**
- No requirements, executive docs, UI/code, dedicated chain route/page, or duplicate persisted fields for latest/lifecycle/topology were added.
- The new spec text stays at behavioral projection level only; it does not invent TypeScript wire implementation details or client-authority contracts.

**Commit**
- `47467a66579e9dd205dac6ed32d173f9b066db4d`
