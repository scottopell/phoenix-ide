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


## Completion evidence

**Files changed**
- `specs/work-lifecycle/requirements.md`
- `specs/work-lifecycle/work-lifecycle.allium`
- `specs/pr-association/requirements.md`
- `tasks/92027-p1-in-progress--workscope-retirement-loss-classification.md`

**Settled facts encoded**
- `specs/work-lifecycle/requirements.md` now makes retirement inspection explicit for attached Git-backed `WorkScope`s, including the exact independent loss categories: staged tracked paths; unstaged tracked/conflicted/unmerged tracked paths; untracked non-ignored paths; initialized-submodule dirty/untracked state; and detached commits unreachable from `refs/heads/*`, `refs/remotes/*`, `refs/tags/*`, or `refs/stash`.
- The same requirements now state the exclusion/inclusion boundaries requested by the task: ignored paths excluded; LFS edits treated as ordinary tracked changes; local/unpushed branches, tags, remotes, and stash treated as durable refs rather than loss; reflog-only detached commits treated as at risk; no recursive nested-repo preservation beyond declared submodules.
- `REQ-WL-002a` now binds discard confirmation to one exact inspection generation + workspace fingerprint and requires reinspection when the workspace changes; it also states that chat-only / no-attached-worktree conversations skip worktree-loss confirmation.
- `REQ-WL-002b` now defines WorkScope retirement ownership and boundaries: continuation members share one live WorkScope; sub-agents may attach but do not gain cleanup authority; retirement is stepwise/idempotent; already-absent worktrees are accepted only with retained identity/evidence; and retirement creates no branch/tag/commit/stash/patch/diff snapshot or other automatic recovery artifact.
- `specs/work-lifecycle/work-lifecycle.allium` now aligns its behavioral focus with the bedrock retirement boundaries (`RetirementInspectionRequested`, `RetirementInspectionCompletedWithoutConfirmation`, `RetirementInspectionRequiresConfirmation`, `RetirementInspectionChanged`, `ResourceRetirementRequested`, `ResourceRetirementCompleted`, `ResourceRetirementFailed`) instead of the retired Abandon/Mark-as-merged lifecycle.
- `specs/pr-association/requirements.md` now explicitly states that PR freshness/coverage never block Close, retirement inspection, retirement, or ordinary conversation use, preserving PR observation as guidance-only and not lifecycle ownership.

**Validation**
- Command: `allium check specs/work-lifecycle/work-lifecycle.allium specs/bedrock/bedrock.allium`
- Result: exit `1` with no `severity: "error"` diagnostics for `specs/work-lifecycle/work-lifecycle.allium`; remaining diagnostics were warnings/info only (primarily unreachable-trigger/unused-declaration warnings, plus pre-existing bedrock warnings/info).
- Command: `allium check specs/work-lifecycle/work-lifecycle.allium`
- Result: superseded by the joint check above because this file imports `specs/bedrock/bedrock.allium`; the standalone run reported unresolved import/trigger-form errors that disappear when checked with the imported dependency set.

**Review / evidence ledger**
- Self-review: removed obsolete Abandon/Mark-as-merged Allium behavior and replaced it with retirement-inspection / resource-retirement behavior centered on the existing bedrock typed boundaries.
- Self-review: corrected invalid prefixed trigger forms in `work-lifecycle.allium` after the first validation pass by switching to the imported-event names and validating together with `specs/bedrock/bedrock.allium`.
- Self-review: narrowed `pr-association` edits to terminology/authority alignment only; no PR-selection or freshness behavior was reowned by work-lifecycle.

**Speculation avoided**
- No new requirements files, ADRs, code, or cross-cluster artifacts were added.
- No ProductConversation lifecycle, Close-phase ordering, or new bedrock retirement boundary names were redefined here; this task consumed the typed bedrock retirement events already present.
- No branch/ref/PR mutation behavior, automatic backup artifact, or child-collection-in-JSON design was introduced.

**Commit**
- 
