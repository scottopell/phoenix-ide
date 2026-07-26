Child of task 92018.

# Durable Close orchestration and cancellation contract across bedrock + durable-workflows

## Objective
Author the durable Close orchestration and cancellation behavior precisely across the bedrock and durable-workflow Allium/spec surfaces, without re-expanding into unrelated lifecycle areas.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/durable-workflows/requirements.md`
- any existing `specs/durable-workflows/*.allium` files if they already exist and are the minimal place to encode this contract
- read-only context from `specs/bedrock/requirements.md`
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- Product lifecycle is Open/History only; Close is the action that transitions the conversation into History.
- Close is durable behavior, not a UI-only gesture, and must be specified where the durable orchestration/cancellation truth already lives.
- Context continuation remains one product conversation; Close semantics must not invent a separate lifecycle for continued rows.
- Latest-row authority remains derived from continuation topology; do not add duplicate ownership for “which row closes”.
- This task owns orchestration/cancellation only. It does not own WorkScope retirement classification, approved-task placement, or History/SSE projection.

## Required work contract
- Keep scope to durable Close orchestration/cancellation behavior across bedrock + durable-workflows only.
- Encode only settled contracts that already have implementation/requirement authority; if a needed helper/contract is not already grounded, narrow the behavior instead of inventing it.
- Do **not** add speculative workflow phases, placeholder events, guessed cancellation APIs, or cross-spec obligations owned by other child tasks.
- Preserve artifact voice: Allium for behavior, requirements only if needed to align existing durable-workflow normative text.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/projects/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- broad bedrock lifecycle topology beyond what Close orchestration needs
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete requirements/Allium rules
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work
