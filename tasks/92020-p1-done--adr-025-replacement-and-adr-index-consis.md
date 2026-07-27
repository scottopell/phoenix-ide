Child of task 92010.

# Replace or amend ADR-025 and reconcile the shared ADR chain

## Objective
Correct the ADR layer so it explains the settled lifecycle/ownership decisions without falsifying ADR history or freezing an unaccepted draft as accepted truth.

## Exact target artifact clusters
- `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`
- `specs/adrs/README.md`
- any newly added replacement/superseding ADR file under `specs/adrs/`
- any spec files whose ADR references must be corrected because the accepted rationale moved

## Settled facts this task MUST encode
- ADR-025 is an **unaccepted draft direction to replace or amend before acceptance**, not a frozen accepted historical artifact to rewrite in place as though the decision were already settled forever.
- Do not rewrite an accepted historical ADR as if its original context never existed.
- If ADR-025 is superseded, replacement must preserve the ADR chain honestly: state context, decision, status, and consequences with accurate chronology.
- Product conversation owns Open/History lifecycle; `WorkScope` owns resources.
- Continuation keeps one product conversation and one attached `WorkScope` across durable rows.
- Use Git-backed vs chat-only; avoid “project conversation” in ADR rationale unless describing a rejected/legacy term explicitly.

## Required work contract
- Decide whether the correct move is to amend ADR-025 as draft material, replace it with a new ADR, or supersede it with a later numbered ADR.
- Update `specs/adrs/README.md` so numbering, titles, statuses, and dependency tree remain internally consistent.
- Ensure ADR prose explains why lifecycle belongs to product conversation while resource ownership belongs to `WorkScope`.
- Keep normative requirements language in requirements; ADRs explain rationale and tradeoffs only.
- Grep for stale ADR references after editing and fix broken backlinks in touched spec files.

## Out of scope
- No code changes.
- Do not treat ADRs as status tables for implementation completeness.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact ADR/index/reference paths
- **Decisions captured** — whether ADR-025 was amended, replaced, or superseded and why
- **Validation** — README/index/reference grep checks run
- **Review corrections** — history/numbering/backlink fixes made after review, or `None`
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`
- `specs/adrs/README.md`
- `tasks/92020-p1-in-progress--adr-025-replacement-and-adr-index-consis.md`

**Decisions captured**
- Rewrote ADR-025 in place and accepted it honestly because it was still an unaccepted branch-local draft, not frozen accepted history.
- Replaced the incorrect "WorkScope-owned lifecycle" framing with an explicit dimensional split: product conversation owns Open/History lifecycle, `Conversation` rows own transcript topology through `continued_in_conv_id`, and `WorkScope` owns resources only.
- Recorded the required rejected alternatives explicitly: row-owned lifecycle, `WorkScope` owning product lifecycle, branch/task/PR ownership, task spawn as continuation, and duplicate root/latest authority.
- Preserved the intentionally undecided point that chat-only conversations may or may not have non-Git `WorkScope` attachments.
- Updated the ADR index title, routing row, and dependency tree wording to match the accepted decision and keep task-routing language aligned.

**Validation**
- Read-first artifacts reviewed: `specs/AUTHORING.md`, `specs/adrs/README.md`, `specs/adrs/008_multi-pr-selection-uses-durable-branch-observations.md`, `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`, parent `tasks/92009-p1-in-progress--unify-conversation-workstream-lifecycle.md`, done children `tasks/92017-p1-done--terminology-authority-requirements-basel.md`, `tasks/92018-p1-done--lifecycle-close-allium-behavior.md`, `tasks/92025-p1-done--bedrock-root-lifecycle-continuation-topology.md`, `tasks/92026-p1-done--durable-close-orchestration-cancellation-contract.md`, `tasks/92027-p1-done--workscope-retirement-loss-classification.md`, `tasks/92028-p1-done--approved-task-placement-behavior.md`, `tasks/92029-p1-done--history-sse-projection.md`, and `tasks/92030-p1-done--cross-file-allium-review.md`.
- Requirement/ADR trace review:
  - `rg -n 'REQ-BED-019|REQ-BED-028|REQ-BED-029|REQ-BED-030|REQ-PROJ-004|REQ-PROJ-015|REQ-PROJ-WS-001|REQ-WL-001|REQ-WL-002|REQ-PRA-000|REQ-CHN-008|REQ-GR-001' specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md specs/adrs/README.md`
- Spec-shape / task validation:
  - `./dev.py check --lanes spec-shape`
  - `./dev.py tasks validate`
- Timelessness / authoring grep:
  - `rg -n 'task [0-9]{3,}|tasks/[0-9]|PR #|see #[0-9]|RESOLVED [0-9]|Open Question|Q[0-9]\. |Progress:|Status Summary|✅|currently|for now|at the moment|recently|previously|landed|MVP|rollout|stopgap' specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md specs/adrs/README.md`
- Independent dimensional self-review performed against the required separations: lifecycle vs transcript topology vs resource ownership vs branch/PR observation.

**Review corrections**
- Replaced an initial invalid attempt to run a non-existent `scripts/spec_shape_check.py` with the repo-supported `./dev.py check --lanes spec-shape` check.
- Confirmed the only timelessness grep hit was the pre-existing README phrase `frozen at the moment it was made`, which is an ADR-chain convention statement rather than drift in ADR-025.

**Commit**
- Pending
