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
- Lifecycle authority is now structurally singular on `ProductConversation.lifecycle`; every durable `Conversation` row references exactly one `product_conversation`, and `Parent.product_lifecycle` derives from that aggregate rather than from a row-local lifecycle field.
- Continuation topology stays single-source: `continued_in_conv_id` remains the only persisted row-ordering edge, `Parent.is_latest_row` is derived solely from successor absence, and no persisted root/predecessor/latest helper fields remain.
- `context_exhausted` and `handed_off` remain row-local parent statuses while the aggregate is open, but no invariant now forces those rows to stay open forever after the aggregate transitions to history.
- `UserStartsContinuationConversation` keeps context continuation inside the same `product_conversation`, preserving one aggregate across linked transcript rows without inventing lifecycle authority on attached work scope or on individual rows.
- `UserApprovesTaskFreshWorkConversation` now explicitly creates a fresh work conversation in a different `product_conversation`; it does not write `continued_in_conv_id`, does not set `handed_off`, and therefore does not model task spawn as continuation.
- Bedrock now models only the aggregate lifecycle capability `ProductConversationTransitionsToHistory`; the fake `CloseConversationCompleted` row event and completed-operation orchestration were removed from this child.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium`
- Result: passed with exit code `1` only because the existing file still emits warnings/info diagnostics; no `severity: "error"` diagnostics were produced and the structural changes validated successfully.

**Review / evidence ledger**
- Self-review: confirmed invalid mixed lifecycle is no longer representable because `Conversation` rows no longer carry a lifecycle field; only `ProductConversation` does.
- Self-review: confirmed task-spawn is not continuation because `UserApprovesTaskFreshWorkConversation` creates `fresh_work_conversation.product_conversation != conversation.product_conversation`, leaves `fresh_work_conversation.continued_in_conv_id = absent`, and does not set `conversation.parent_status = handed_off`.
- Self-review correction round: removed the prior root/predecessor-based topology model, replaced it with singular aggregate membership plus derived latest-row authority, and replaced the fake close-completed trigger with aggregate-scoped `ProductConversationTransitionsToHistory`.
- Reviewer findings addressed:
  - Mixed lifecycle representability fixed by moving lifecycle from `Conversation` to `ProductConversation`.
  - Row-local `handed_off` / `context_exhausted` no longer imply permanently-open rows after aggregate history.
  - Fresh task approval no longer shares continuation topology or aggregate root.
  - Work scope is no longer described as lifecycle authority.

**Speculation avoided**
- No speculative provenance helper, root/predecessor cache, lifecycle authority on work scope, or downstream close-orchestration contract was added; the only new structural element is the singular `ProductConversation` aggregate required to make mixed lifecycle unrepresentable.

**Commit**
- `a2b1ad73d6de640d6c8483e0bd4f33910943c0f5`
