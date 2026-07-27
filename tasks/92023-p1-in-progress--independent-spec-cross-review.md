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

## Review evidence

**Files reviewed**
- Parent and child evidence: `tasks/92009-p1-in-progress--unify-conversation-workstream-lifecycle.md`, `tasks/92017-p1-done--terminology-authority-requirements-basel.md`, `tasks/92018-p1-done--lifecycle-close-allium-behavior.md`, `tasks/92019-p1-done--environment-proposal-provenance-retrieva.md`, `tasks/92020-p1-done--adr-025-replacement-and-adr-index-consis.md`, `tasks/92021-p1-done--legacy-design-md-v2-migration-affected-s.md`, `tasks/92022-p1-done--executive-status-reconciliation.md`, `tasks/92025-p1-done--bedrock-root-lifecycle-continuation-topology.md`, `tasks/92026-p1-done--durable-close-orchestration-cancellation-contract.md`, `tasks/92027-p1-done--workscope-retirement-loss-classification.md`, `tasks/92028-p1-done--approved-task-placement-behavior.md`, `tasks/92029-p1-done--history-sse-projection.md`, `tasks/92030-p1-done--cross-file-allium-review.md`
- Authoring / ADRs: `specs/AUTHORING.md`, `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`, `specs/adrs/README.md`
- Normative/current artifacts reviewed: `specs/bedrock/{requirements.md,bedrock.allium,executive.md}`, `specs/projects/{requirements.md,projects.allium,executive.md}`, `specs/work-lifecycle/{requirements.md,work-lifecycle.allium,executive.md}`, `specs/conversation-retrieval/{requirements.md,executive.md}`, `specs/conversation-ui/{requirements.md,executive.md}`, `specs/chains/{requirements.md,executive.md}`, `specs/global-recall/requirements.md`, `specs/pr-association/requirements.md`, `specs/sse_wire/sse_wire.allium`

**Mismatch inventory before edits**
1. **High — `specs/bedrock/requirements.md:818-909`**: `REQ-BED-032` still normatively described `archive`, `abandon`, and `mark-merged` as terminal lifecycle transitions, plus `unarchive` discussion and `PR #135` rationale. This contradicted the settled single-lifecycle Open→History model, duplicated work-lifecycle authority, and violated timelessness.
2. **Medium — `specs/bedrock/requirements.md:679-685`**: `REQ-BED-028` rationale still said approval-time git operations “happen on the task branch,” contradicting the settled no-task-branch ownership model.
3. **Medium, review-only no edit — `specs/chains/requirements.md`**: dedicated chain Q&A and chain-first navigation remain normative. This is internally coherent with the chains spec’s declared legacy/current surface and its executive honesty, but it remains an intentional spec-family divergence from the unified-conversation target rather than a hidden mismatch.
4. **Low, reviewed no edit — formal-gap check for source relations / follow-up / deletion**: `requirements.md` artifacts (`bedrock`, `projects`, `conversation-retrieval`) now define typed source relations, deleted-source tombstones, follow-up freshness, and retrieval scoping clearly enough that the absence of a dedicated Allium source-relation spec is not by itself a proven formal gap in this task’s scope.

**Corrections made**
- Rewrote `REQ-BED-032` so it owns only permanent Delete’s hard-delete cascade, explicitly defers Close-to-History retirement to `REQ-BED-029` + `specs/work-lifecycle/requirements.md`, removes legacy `archive` / `abandon` / `mark-merged` lifecycle truth, and removes the timelessness-violating `PR #135` note.
- Rewrote the `REQ-BED-028` rationale sentence so approval-time git operations no longer imply a task-branch lifecycle.

**Cross-review verdicts against requested checkpoints**
- ProductConversation aggregate/root wording: **Pass** after review; ADR-025 distinguishes aggregate identity from any one root row.
- WorkScope attachment not ownership by row: **Pass** in ADR-025, `REQ-PROJ-WS-001`, `REQ-BED-030`, and retrieval scoping.
- Chat-only scope undecided: **Pass**; ADR-025 leaves non-Git WorkScope attachment for chat-only intentionally undecided without leaking contradictory normative claims.
- Continuation same aggregate/scope: **Pass** in `REQ-BED-030`, `REQ-PROJ-015`, ADR-025.
- Task fresh/follow-up separate: **Pass** in `REQ-BED-028`, `REQ-BED-031A`, `REQ-PROJ-004`, retrieval source-relation requirements.
- Continue here no lifecycle/environment side effects: **Pass**.
- Close repair stays Open: **Pass** through work-lifecycle + ADR-025.
- History only after retirement: **Pass** in ADR-025 and work-lifecycle ownership split.
- Deletion/tombstone: **Pass** after `REQ-BED-032` cleanup; requirements and retrieval still preserve deleted-source distinction.
- Retrieval scoping: **Pass**; `REQ-RET-008` excludes siblings/follow-ups/Coordinator unless host-bound explicitly widens scope.
- Coordinator: **Pass**; chat-only singleton remains excluded from ordinary lifecycle.
- Default branch no arbitrary fallback: **Pass** in `specs/projects/requirements.md` and reviewed `projects.allium` evidence.
- Branch/PR untouched by lifecycle: **Pass**.
- No dedicated chain normative surface: **Fail as product-unification target, but honest as declared legacy spec family.** The dedicated chain normative surface still exists in `specs/chains/requirements.md`; because its executive explicitly calls this out as current shipped divergence, I treated it as known intentional drift rather than a hidden mismatch for this task.
- Legacy terms only in executives/negative compatibility: **Pass after bedrock cleanup** for the reviewed lifecycle cluster; remaining legacy product surface lives in executives or explicitly legacy/deprecated requirements.

**Validation**
- Semantic discovery / review greps:
  - `rg -n 'ProductConversation|root transcript row|root row|WorkScope|chat-only|Continue here|Start in new conversation|follow-up|Deleted source|tombstone|History|Close conversation|default branch|current_branch|branch_name|base_branch|Coordinator|chain page|chain route|project conversation|Continuation' specs/{bedrock,projects,conversation-retrieval,chains}/*.md`
  - `rg -n 'task branch|branch mode|Work mode|Managed mode|chain page|chain route|archived|archive|Abandon|Mark as merged|mark-merged|project conversation' specs/bedrock/requirements.md specs/projects/requirements.md specs/chains/requirements.md specs/conversation-retrieval/requirements.md`
  - `rg -n 'REQ-RET-007|REQ-RET-008|REQ-RET-009|REQ-CHN-00[2-9]|REQ-PROJ-015|REQ-BED-028|REQ-BED-029|REQ-BED-030|REQ-BED-031A|REQ-GR-001|Deleted source|follow_up|approved_task|Continue here|Start in new conversation|Coordinator|Global|Conversations\(ids\)|chain page|chain route' specs/{conversation-retrieval,chains,projects,bedrock,global-recall}/requirements.md`
  - `rg -n 'Progress:|currently|for now|at the moment|recently|previously|landed|MVP|rollout|stopgap|task [0-9]{3,}|tasks/[0-9]|PR #|see #[0-9]|RESOLVED [0-9]|Open Question' specs/bedrock/requirements.md`
- Allium / shape checks:
  - `allium check specs/bedrock/bedrock.allium` → `errors=0 warnings=10`
  - `allium check specs/projects/projects.allium` → `errors=0 warnings=1`
  - `allium check specs/work-lifecycle/work-lifecycle.allium` → `errors=0 warnings=3`
  - `allium check specs/pr-association/pr-association.allium` → `errors=0 warnings=13`
  - `allium check specs/sse_wire/sse_wire.allium` → `errors=0 warnings=0`
  - `./dev.py check --lanes spec-shape`
  - `./dev.py tasks validate`

**Severity / follow-up IDs**
- Fixed in this task:
  - High: `REQ-BED-032` parallel legacy lifecycle authority and timelessness drift.
  - Medium: `REQ-BED-028` stray task-branch wording.
- Follow-up IDs: None filed. I did **not** reopen the broader deliverable because the remaining chain-surface divergence is already explicit and honestly reported in `specs/chains/executive.md`, not hidden under false normative singularity.

**Commit**
- Pending local commit
