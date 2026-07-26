Child of task 92018.

# Approved-task placement behavior across bedrock + projects

## Objective
Specify the exact approved-task placement behavior across the bedrock and projects spec cluster, limited to how approved work resumes in place versus starts in a separate conversation.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/bedrock/requirements.md` only if a normative clarification is strictly required to match the settled facts already accepted in 92017
- `specs/projects/requirements.md`
- any existing `specs/projects/*.allium` files if they already own approved-task placement behavior
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- `Continue here` resolves approved-task review and resumes the same conversation context.
- `Continue here` has no lifecycle, provenance, `WorkScope`, worktree, branch, or PR side effect.
- `Start in new conversation` creates a separate Open conversation with fresh `WorkScope`/worktree, exact approved task only, and visible Derived from provenance.
- Follow-up is separate and fresh rather than a continuation of the origin.
- This task owns approved-task placement behavior only; it must not absorb root lifecycle topology, durable Close orchestration, WorkScope retirement classification, or seamless History/SSE projection.

## Required work contract
- Keep the contract focused on approved-task placement across bedrock + projects.
- Encode only settled placement behavior already grounded by 92017/92018 facts and current requirements/code reality.
- Do **not** add speculative review states, guessed UI flows, placeholder provenance helpers, or unrelated lifecycle/resource obligations.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/durable-workflows/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete rules/requirements
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work
