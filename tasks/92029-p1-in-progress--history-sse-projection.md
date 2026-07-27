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
