Child of task 92018.

# Cross-file Allium review for lifecycle contract split after child tasks land

## Objective
Run the final cross-file review after the five scoped lifecycle child tasks land, verifying that the split contracts compose cleanly and that no child reintroduced the overbroad/speculative failure mode.

## Exact target artifacts
Review the exact artifacts changed by the five children of 92018 covering:
- `specs/bedrock/bedrock.allium`
- `specs/durable-workflows/**` touched by the Close orchestration child
- `specs/projects/**` touched by the WorkScope retirement or approved-task placement children
- `specs/work-lifecycle/**` touched by the WorkScope retirement child
- `specs/pr-association/**` touched by the WorkScope retirement child
- `specs/sse_wire/**` touched by the History/SSE projection child
- the five completed child task files themselves for completion evidence

## Settled facts from 92017 / 92018 that this review MUST enforce
- Product lifecycle is Open/History only; Close is the action and History the resulting state.
- Context continuation is one product conversation across `continued_in_conv_id` rows and one attached `WorkScope` unless a separate new conversation is explicitly created.
- Latest-row authority is derived, not duplicated.
- `Continue here` has no lifecycle/provenance/WorkScope/worktree/branch/PR side effect.
- `Start in new conversation` is separate, fresh, exact-approved-task-only, and visibly derived.
- `WorkScope` retirement/loss classification stays separate from conversation lifecycle ownership.
- History/SSE projection stays seamless and does not invent split authority.

## Required work contract
- Do not start until the five scoped child tasks are complete.
- Review cross-file composition, helper/import hygiene, and artifact-boundary discipline.
- Verify every edited `.allium` file with `allium check`, even if the owning child already ran it.
- Verify each child task appended the required evidence ledger sections.
- File concrete follow-up tasks for any remaining drift rather than broad “future work” notes.
- Do **not** rewrite requirements or Allium opportunistically unless a minimal review correction is necessary and directly evidenced.

## Out of scope
- New lifecycle redesign
- code changes
- speculative helpers/contracts beyond review fixes

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files reviewed** — exact paths/clusters reviewed
- **Settled facts enforced** — bullets listing each cross-file rule checked and verdict
- **Validation** — exact `allium check` commands plus pass/fail output summary for every edited `.allium`, and `./dev.py tasks validate`
- **Review / evidence ledger** — review findings, minimal corrections made, and follow-up task IDs filed, or `None`
- **Speculation avoided** — explicit note that the review rejected speculative helpers/contracts or parallel authorities
- **Commit** — commit hash that landed review corrections, or `None` if review-only
