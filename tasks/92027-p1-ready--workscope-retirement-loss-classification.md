Child of task 92018.

# WorkScope retirement and loss classification across projects + work-lifecycle + pr-association

## Objective
Specify only the retirement/loss-classification behavior for `WorkScope` across the projects, work-lifecycle, and pr-association spec cluster, separating resource retirement truth from conversation lifecycle truth.

## Exact target artifacts
- `specs/projects/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/pr-association/requirements.md`
- any existing `.allium` files under those spec directories if, and only if, they already own this behavior
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- `WorkScope` owns resources; the product conversation owns lifecycle.
- Context continuation keeps one attached `WorkScope`; continuation does not create fresh work ownership.
- Follow-up / Start in new conversation creates separate fresh work ownership; Continue here does not.
- Branches and PRs are observed repository facts, not lifecycle-owned artifacts Phoenix mutates.
- This task is about retirement/loss classification of `WorkScope` and related observed repository state only; it must not redefine Close/History, root continuation topology, approved-task placement, or SSE/history projection.

## Required work contract
- Limit edits to the projects/work-lifecycle/pr-association cluster.
- Make retirement/loss classification explicit where that cluster already has normative authority.
- Do **not** add speculative cleanup actors, guessed branch-mutation guarantees, placeholder state machines, or invented helpers/contracts.
- If any `.allium` file is edited, run `allium check` on each edited file and record exact commands/results.
- Preserve the separation: conversation lifecycle in bedrock, resource retirement/loss classification here.

## Out of scope
- `specs/bedrock/**`
- `specs/durable-workflows/**`
- `specs/sse_wire/**`
- approved-task placement behavior
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete rules/requirements
- **Validation** — exact `allium check` commands run for any edited `.allium`, or `No Allium files edited`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work
