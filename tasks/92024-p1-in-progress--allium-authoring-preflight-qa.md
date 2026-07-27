Child of task 92010.

# Run AUTHORING preflight and validation on the lifecycle spec workset

## Objective
Use `specs/AUTHORING.md` as the final gate for the lifecycle-spec workset after authoring tasks land, with explicit checks for terminology drift, Allium validity, cross-artifact agreement, and task hygiene.

## Exact target artifact clusters
- every spec artifact changed by tasks 92017-92023
- `specs/AUTHORING.md` checklist items applicable to that touched set
- task files if follow-up tasks are created during QA

## Settled facts this QA MUST enforce
- Product conversation lifecycle is Open/History only.
- Close conversation is the action; History is the resulting state; QA should fail any artifact that uses “Closed” as lifecycle truth.
- Context continuation is one product conversation across multiple durable rows linked by `continued_in_conv_id` and one attached `WorkScope`.
- `WorkScope` owns resources; conversations may attach to one; sub-agent/execution conversations may share it without cleanup authority.
- Git-backed vs chat-only is the preferred distinction; reject revived “project conversation” terminology unless clearly marked legacy/current-compat.
- Latest-row authority derives from `continued_in_conv_id`, not from duplicated parallel fields or prose authority.

## Required work contract
- Run the relevant `specs/AUTHORING.md` checks against the touched spec set, not a hand-wavy subset.
- Run `allium check` for every affected Allium file.
- Run `./dev.py tasks validate` before declaring the workset ready.
- Verify that completion evidence was appended to the child task bodies that were executed.
- If a checklist item fails, either fix it in the owning spec task or file a concrete follow-up; do not hide it in vague notes.

## Out of scope
- No code changes.
- No status renames for these task files as part of this task body itself.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files covered** — exact spec/task paths included in QA scope
- **Decisions captured** — checklist items that mattered and notable pass/fail judgments
- **Validation** — exact commands run, including `allium check` and `./dev.py tasks validate`
- **Review corrections** — fixes or follow-up tasks created from QA, or `None`
- **Commit** — commit hash that landed the QA-driven corrections, or `None` if review-only
