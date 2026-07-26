Child of task 92018.

# Bedrock-only Allium for root lifecycle and continuation topology

## Objective
Author only the root conversation lifecycle and continuation-topology behavior in `specs/bedrock/bedrock.allium`, recovering the abandoned overbroad attempt by shrinking scope to bedrock-owned facts that are already settled.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- read-only context from `specs/bedrock/requirements.md`
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- Product conversation lifecycle is **Open** or **History** only.
- **Close conversation** is the operation; **History** is the resulting state. Do not define **Closed** as a lifecycle label.
- A product conversation is identified by the durable root, even when multiple durable transcript rows are linked by `continued_in_conv_id`.
- Context continuation is one product conversation spanning those linked rows; it does not create a second product lifecycle.
- Latest-row authority is derived from `continued_in_conv_id` topology and durable rules; do not invent a second "latest" owner, cache, helper contract, or parallel authority field.
- A conversation may attach to a `WorkScope`, but lifecycle belongs to the product conversation, not to `WorkScope` and not to an individual transcript row.
- This recovery phase is limited to bedrock-owned lifecycle/topology behavior only; do not absorb durable Close orchestration, task approval placement, History/SSE projection, or WorkScope retirement semantics into this task.

## Required work contract
- Modify only `specs/bedrock/bedrock.allium`.
- Encode root lifecycle states, continuation topology, and latest-row derivation only where bedrock already has settled authority.
- Use only helpers/contracts/imports that are already present or are strictly necessary to express this exact behavior in the same file.
- Do **not** add speculative helpers, placeholder contracts, TODO entities, guessed downstream projections, or cross-file obligations owned by other child tasks.
- Keep rationale/status/history out of Allium; write behavioral rules only.
- Run `allium check specs/bedrock/bedrock.allium` and record the result in the evidence ledger.

## Out of scope
- `specs/durable-workflows/**`
- `specs/projects/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- code, ADRs, executive docs, and requirements edits

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete Allium rules/entities
- **Validation** — exact `allium check specs/bedrock/bedrock.allium` command plus pass/fail output summary
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `tasks/92025-p1-in-progress--bedrock-root-lifecycle-continuation-topology.md`

**Settled facts encoded**
- Product lifecycle authority now lives on the durable root conversation via `Conversation.lifecycle`, with `Parent.product_lifecycle` derived through `root_conversation` and enforced by `RootLifecycleAuthority`.
- Continuation topology is modeled structurally with `Conversation.predecessor`, `Conversation.root`, and existing `continued_in_conv_id`; `LatestRowDerivedFromTopology` makes latest-row authority derive from successor absence rather than a persisted latest/current field.
- `Parent.parent_status = context_exhausted | handed_off` remains a row condition inside an open product conversation via `ContinuedRowStaysOpenUntilRootHistory`; handed-off predecessors stay read-only without becoming History.
- Continuation creation preserves the same open product lifecycle and attached execution environment in `UserStartsContinuationConversation`, while fresh task approval remains a separate conversation lineage in `UserApprovesTaskFreshWorkConversation`.
- Close orchestration was not invented; bedrock only models the lifecycle state flip in `ConversationLifecycleBecomesHistory`, gated to the latest parent row and applied to `root_conversation.lifecycle`.
- Contradictory terminal/archive/task-resolution behavior was removed from the bedrock lifecycle contract by replacing `TaskResolved -> terminal` / `ConversationReachedTerminalState` with root-lifecycle History rules only.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium`
- Result: passes with `errors 0`; remaining diagnostics are pre-existing warnings/info from the broader file, not validation errors.

**Review / evidence ledger**
- Self-review: corrected `UserApprovesTaskFreshWorkConversation` so the fresh conversation roots to `conversation.root_conversation` rather than itself, preserving one durable root authority across the topology.
- Self-review: verified `ConversationLifecycleBecomesHistory` changes only the root-owned product lifecycle and does not introduce Close orchestration.
- Reviewer findings: None.

**Speculation avoided**
- No speculative imports, cross-file contracts, persisted latest/current field, or Close-orchestration rules were added; edits stayed inside `specs/bedrock/bedrock.allium` and bedrock-owned lifecycle/topology behavior.

**Commit**
- `8c651be0f4c4de8258393713fc46c988426ac7c7`
