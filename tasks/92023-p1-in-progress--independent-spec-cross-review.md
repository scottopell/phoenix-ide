Child of task 92010.

# Cross-review lifecycle spec clusters for drift after authoring lands

## Objective
Perform an independent review pass over the edited lifecycle-related spec set so contradictions, traceability gaps, and terminology regressions are caught before implementation begins.

## Exact target artifact clusters
Review every spec artifact changed by tasks 92017-92022, including as applicable:
- `requirements.md`
- `.allium`
- `executive.md`
- `specs/adrs/*.md`
- any touched legacy `design.md` files left as source material with documented reasons

## Settled facts this review MUST enforce
- Product conversation lifecycle is Open/History only.
- Close conversation is the action; History is the resulting state; no artifact should reintroduce “Closed” as the lifecycle name.
- Context continuation remains one product conversation and one attached `WorkScope` across `continued_in_conv_id` rows.
- `WorkScope` owns resources; conversations may attach to one; sub-agent/execution conversations may share it without cleanup authority.
- `Continue here` has no lifecycle/provenance/WorkScope/worktree/branch/PR side effect.
- `Start in new conversation` creates a separate Open conversation, fresh `WorkScope`/worktree, exact approved task only, and visible Derived from source.
- Follow-up is separate and fresh; branches/PRs are observed, not owned.
- Latest-row authority derives from `continued_in_conv_id`; no duplicate “latest” authority is allowed.

## Required work contract
- Review artifact-by-artifact, not just via broad impressions.
- Verify each artifact type stays in its proper voice: requirements timeless, Allium behavioral, ADR rationale, executive status/current reality.
- Check that cross-references among REQs, Allium rules, and ADR IDs still resolve after edits.
- File concrete follow-up tasks for any gap that should not be fixed in-place.
- Record verdicts per artifact cluster, not a single generic “looks good”.

## Out of scope
- No speculative redesign.
- No code changes.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files reviewed** — exact artifact paths/clusters checked
- **Decisions captured** — key drift calls and why they passed or failed
- **Validation** — grep/check commands used during review
- **Review corrections** — fixes made or follow-up tasks filed, with IDs if created, or `None`
- **Commit** — commit hash that landed any direct review corrections, or `None` if review-only
