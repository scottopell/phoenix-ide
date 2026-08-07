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


## Completion evidence

**Files covered**
- Checklist and parent/task evidence: `specs/AUTHORING.md`, `tasks/92010-p1-in-progress--conversation-lifecycle-spec-schema.md`, `tasks/92017-p1-done--terminology-authority-requirements-basel.md`, `tasks/92018-p1-done--lifecycle-close-allium-behavior.md`, `tasks/92019-p1-done--environment-proposal-provenance-retrieva.md`, `tasks/92020-p1-done--adr-025-replacement-and-adr-index-consis.md`, `tasks/92021-p1-done--legacy-design-md-v2-migration-affected-s.md`, `tasks/92022-p1-done--executive-status-reconciliation.md`, `tasks/92023-p1-done--independent-spec-cross-review.md`, `tasks/92025-p1-done--bedrock-root-lifecycle-continuation-topology.md`, `tasks/92026-p1-done--durable-close-orchestration-cancellation-contract.md`, `tasks/92027-p1-done--workscope-retirement-loss-classification.md`, `tasks/92028-p1-done--approved-task-placement-behavior.md`, `tasks/92029-p1-done--history-sse-projection.md`, `tasks/92030-p1-done--cross-file-allium-review.md`
- Normative + rationale artifacts audited in scope: `specs/bedrock/{requirements.md,bedrock.allium,executive.md}`, `specs/projects/{requirements.md,projects.allium,executive.md}`, `specs/work-lifecycle/{requirements.md,work-lifecycle.allium,executive.md}`, `specs/conversation-retrieval/{requirements.md,executive.md}`, `specs/conversation-ui/{requirements.md,executive.md}`, `specs/chains/{requirements.md,executive.md}`, `specs/pr-association/{requirements.md,pr-association.allium,executive.md}`, `specs/global-recall/requirements.md`, `specs/durable-workflows/{requirements.md,durable-workflows.allium,wake-profile.allium,creation-profile.allium}`, `specs/sse_wire/{sse_wire.allium,executive.md}`, `specs/subagents/subagents.allium`, `specs/adrs/{README.md,025_workscope-owned-lifecycle-unifies-conversation-handoffs.md}`

**Decisions captured**
- AUTHORING checklist gates passed for this workset: per-file Allium validation had zero `severity: "error"` findings across all affected `.allium` files; `./dev.py check --lanes spec-shape,spec-anchors,allium`, `./dev.py tasks validate`, and `git diff --check` all passed.
- Combined Allium run also produced zero error-severity diagnostics; the CLI still returned non-zero because warning/info diagnostics remain on several files. That is acceptable under the checklist's zero-error bar and was verified by parsing JSON diagnostics rather than trusting exit code alone.
- REQ semantic authority audit passed for the deliverable scope: Open/History remains the singular product lifecycle; `continued_in_conv_id` remains the only continuation/latest topology authority; `WorkScope` remains the resource owner rather than lifecycle owner; approved-task provenance uses typed `approved_task`; follow-up uses typed `follow_up`; retrieval and tombstone requirements preserve deleted-source distinction; Coordinator remains excluded from ordinary conversation lifecycle/WorkScope behavior.
- ADR-025 / ADR index audit passed: ADR-025 remains accepted history rather than rewritten pseudo-history, its `Affects:` set is present in the ADR row and README row, and README links/titles/statuses are internally consistent.
- Removed-design-reference audit for this deliverable passed at the targeted lifecycle workset level: no active lifecycle task/spec artifact in scope still treats the removed lifecycle design docs as current authority. Residual `design.md` references elsewhere in the repo remain broader migration debt, not a blocker for this package.
- One non-blocking repo-wide warning was verified, not fixed here: `REQ-WAKE-012` is duplicated within `specs/wake-contracts/requirements.md`, outside the 92010/92024 lifecycle deliverable scope.
- One implementation-drift warning remains as previously documented in child evidence and is not a spec blocker: code paths may still perform Continue-here mode upgrade / fresh-continuation persistence behavior inconsistent with the reviewed spec package.

**Validation**
- Read-first / evidence review:
  - `read specs/AUTHORING.md`
  - `read tasks/92010-p1-in-progress--conversation-lifecycle-spec-schema.md`
  - `read tasks/92017-p1-done--terminology-authority-requirements-basel.md`
  - `read tasks/92018-p1-done--lifecycle-close-allium-behavior.md`
  - `read tasks/92019-p1-done--environment-proposal-provenance-retrieva.md`
  - `read tasks/92020-p1-done--adr-025-replacement-and-adr-index-consis.md`
  - `read tasks/92021-p1-done--legacy-design-md-v2-migration-affected-s.md`
  - `read tasks/92022-p1-done--executive-status-reconciliation.md`
  - `read tasks/92023-p1-done--independent-spec-cross-review.md`
  - `read tasks/92025-p1-done--bedrock-root-lifecycle-continuation-topology.md`
  - `read tasks/92026-p1-done--durable-close-orchestration-cancellation-contract.md`
  - `read tasks/92027-p1-done--workscope-retirement-loss-classification.md`
  - `read tasks/92028-p1-done--approved-task-placement-behavior.md`
  - `read tasks/92029-p1-done--history-sse-projection.md`
  - `read tasks/92030-p1-done--cross-file-allium-review.md`
- Allium zero-error verification (individual + combined):
  - `python3 - <<'PY' ... subprocess.run(['allium','check', <file>]) ... parse JSON severity counts ... PY`
  - Files checked individually: `specs/bedrock/bedrock.allium`, `specs/projects/projects.allium`, `specs/work-lifecycle/work-lifecycle.allium`, `specs/pr-association/pr-association.allium`, `specs/sse_wire/sse_wire.allium`, `specs/durable-workflows/durable-workflows.allium`, `specs/durable-workflows/wake-profile.allium`, `specs/durable-workflows/creation-profile.allium`, `specs/subagents/subagents.allium`
  - Parsed results: `errors=0` for every file; warnings/info remained on several files
  - Combined command in the same script: `allium check specs/bedrock/bedrock.allium specs/projects/projects.allium specs/work-lifecycle/work-lifecycle.allium specs/pr-association/pr-association.allium specs/sse_wire/sse_wire.allium specs/durable-workflows/durable-workflows.allium specs/durable-workflows/wake-profile.allium specs/durable-workflows/creation-profile.allium specs/subagents/subagents.allium`
  - Combined parsed result: `errors=0`
- Required authoring/check lanes:
  - `./dev.py check --lanes spec-shape,spec-anchors,allium`
  - `./dev.py tasks validate`
  - `git diff --check`
- Additional QA audit commands:
  - `python3 - <<'PY' ... scan specs/*/requirements.md for duplicate REQ definitions / missing definitions ... PY`
  - `rg -n 'REQ-BED-029|REQ-BED-030|REQ-BED-031A|REQ-PROJ-015|REQ-PROJ-WS-001|REQ-WL-001|REQ-WL-002|REQ-PRA-000|REQ-CHN-008|REQ-GR-001' specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`
  - `rg -n 'design\.md' specs tasks`
  - `rg -n 'Continue here|Start in new conversation|follow_up|approved_task|Close conversation|History|continued_in_conv_id|WorkScope|Coordinator|chain route|chain page|project conversation|task branch|archive|mark merged|abandon|Closed' specs/bedrock/bedrock.allium specs/sse_wire/sse_wire.allium`

**Review corrections**
- None. The package passed preflight without a new scoped spec fix.
- Residual warnings recorded instead of broadening scope:
  - `specs/wake-contracts/requirements.md` has a duplicate `REQ-WAKE-012` definition outside this deliverable cluster.
  - Repo-wide legacy `design.md` references still exist in other spec families/tasks; they were not treated as blockers because 92021/92024 only owned the lifecycle cluster.
  - Previously documented implementation drift remains for follow-on implementation gates, not spec QA failure.

**Commit**
- `None` — review-only pass; no artifact changes were required.
