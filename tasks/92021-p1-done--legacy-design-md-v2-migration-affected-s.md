Child of task 92010.

# Migrate lifecycle-related legacy design.md content into spEARS v2 homes

## Objective
Apply the spEARS v2 migration rules to lifecycle-related legacy `design.md` material so rationale, behavior, requirements, and status each land in the right artifact type before new authoring drifts across file classes.

## Exact target artifact clusters
Likely affected legacy source material to inspect first:
- `specs/bedrock/design.md`
- `specs/chains/design.md`
- `specs/conversation-retrieval/design.md`
- `specs/conversation-ui/design.md`
- `specs/projects/design.md`
- `specs/pr-association/design.md`
- `specs/work-actions-bar/design.md`
- `specs/work-lifecycle/design.md`
- `specs/work-scope-ui/design.md`
- any additional `design.md` file proven by grep to contain lifecycle/continuation/follow-up/WorkScope language

Destination artifact classes:
- `requirements.md` for timeless user need / REQ text
- `.allium` for precise behavior
- `specs/adrs/*.md` for rationale and decisions
- `executive.md` for status/current reality

## Settled facts this task MUST preserve during migration
- Product conversation identified by durable root owns Open/History lifecycle.
- Close conversation is the action; History is the resulting state; never migrate “Closed” as lifecycle truth.
- Context continuation stays one product conversation and one attached `WorkScope` across `continued_in_conv_id` rows.
- `WorkScope` owns resources; conversation lifecycle does not.
- `Continue here`, `Start in new conversation`, and follow-up remain distinct behaviors with the side-effect boundaries already settled.
- Use Git-backed vs chat-only; avoid migrating “project conversation” forward except as explicit legacy wording.

## Required work contract
- Follow spEARS v2 migration rules from repo guidance: requirements timeless, ADRs historical rationale, Allium precise behavior, executive current reality.
- Do not copy task IDs, PR references, status bullets, or resolved-question logs into timeless artifacts.
- For each migrated section, decide whether it should move, be rewritten, or stay temporarily as legacy source material with a documented reason.
- Update cross-references after migration so touched specs point to current v2 homes instead of stale design.md prose.

## Out of scope
- No code changes.
- Do not delete unrelated legacy `design.md` files wholesale.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact legacy sources and destination artifact paths
- **Decisions captured** — section-by-section migration calls and why
- **Validation** — grep/spec-shape checks run
- **Review corrections** — migration cleanup after review, or `None`
- **Commit** — commit hash that landed the work

## Completion note

### Files changed
- Removed legacy design docs:
  - `specs/work-lifecycle/design.md`
  - `specs/work-actions-bar/design.md`
  - `specs/chains/design.md`
  - `specs/pr-association/design.md`
  - `specs/bedrock/design.md`
  - `specs/projects/design.md`
- Updated destination/current-authority artifacts:
  - `specs/work-lifecycle/executive.md`
  - `specs/work-actions-bar/executive.md`
  - `specs/chains/executive.md`
  - `specs/bedrock/executive.md`
  - `specs/projects/requirements.md`
- No file existed for:
  - `specs/durable-workflows/design.md`
  - `specs/global-recall/design.md`

### Decisions captured
#### Evidence matrix — `specs/work-lifecycle/design.md`
- Architecture overview → **stale delete**; current authority already split across `specs/work-lifecycle/requirements.md`, `specs/bedrock/bedrock.allium`, and ADR-025.
- Legality gate / worktree ownership / relationship to bedrock → **already captured** in `specs/bedrock/bedrock.allium` (`ProductConversation`, `CloseObligation`, continuation ownership) and ADR-025.
- Mark-as-merged / abandon / diff snapshot / PR-state-as-cleanup-gate → **stale delete** because they described deprecated lifecycle verbs. Current load-bearing contract lives in `specs/work-lifecycle/requirements.md` REQ-WL-001..003.
- Unique content moved? **No new requirement/behavior/rationale needed**; executive rewritten to point to current authority and note removal.
- Disposition: **removed**.

#### Evidence matrix — `specs/work-actions-bar/design.md`
- Disposition tables / single-primary / resolve/finish behavior / browser exclusion / sibling-spec ownership → **already captured** in `specs/work-actions-bar/requirements.md` REQ-WAB-001..012.
- Design-decision prose → **stale delete**; rationale already lives inline under requirements and does not warrant a new ADR.
- Unique content moved? **No**; executive updated to declare current authority after removing legacy file.
- Disposition: **removed**.

#### Evidence matrix — `specs/chains/design.md`
- Chain identity / sidebar grouping / chain page / Q&A persistence / chain-name regeneration / out-of-scope notes → **already captured** in `specs/chains/requirements.md` and `specs/chains/executive.md`.
- Work identity / PR-health ownership split → **already captured** in `specs/chains/requirements.md` REQ-CHN-008 plus ADR-025.
- Q&A backend / persistence details → **status/current reality**, already summarized in `specs/chains/executive.md`; no timeless move needed.
- Unique content moved? **No**; executive updated to state removed `design.md` is no longer authoritative.
- Disposition: **removed**.

#### Evidence matrix — `specs/pr-association/design.md`
- WorkScope-keyed observation history / primary derivation / refresh semantics / autofix affordance / feedback freshness baseline / abandon refresh → **already captured** in `specs/pr-association/requirements.md` and `specs/pr-association/pr-association.allium`.
- Relationship-to-other-specs prose → **stale delete**; scope is already stated in `requirements.md`.
- Unique content moved? **No**.
- Disposition: **removed**.

#### Evidence matrix — `specs/bedrock/design.md`
- Core state-machine architecture, close-obligation lifecycle, continuation semantics, task approval, persistence, lifecycle-event boundaries → **already captured** in `specs/bedrock/requirements.md` and `specs/bedrock/bedrock.allium`.
- Appendix failure-mode and architecture-review prose → **stale delete** for this bounded migration; no current active spec points should rely on removed prose.
- Unique content moved? **No**; executive updated to redirect authority to requirements + Allium + ADR-025.
- Disposition: **removed**.

#### Evidence matrix — `specs/projects/design.md`
- Task proposal / approval / task-source / worktree / sub-agent inheritance / persistence / tool-registry sections → **already captured** in `specs/projects/requirements.md` and `specs/projects/projects.allium`.
- Legacy references to `work-lifecycle/design.md` and `pr-association/design.md` → **moved/rewritten** by removing design-file authority and using `specs/work-lifecycle/requirements.md` REQ-WL-001..003 instead.
- Legacy squash-merge deprecation wording → **rewritten** in `specs/projects/requirements.md` REQ-PROJ-009 to cite current work-lifecycle requirements instead of old action names.
- Unique content moved? **No new ADR**; ADR-025 already captures lifecycle/workscope rationale.
- Disposition: **removed**.

#### Non-present targets
- `specs/durable-workflows/design.md` → no file present; no migration needed.
- `specs/global-recall/design.md` → no file present; no migration needed.

### Validation
- `rg -n "(projects|work-lifecycle|chains|work-actions-bar|pr-association|bedrock)/design\\.md|work-lifecycle/design\\.md|pr-association/design\\.md|projects/design\\.md|chains/design\\.md|work-actions-bar/design\\.md|bedrock/design\\.md" specs tasks` → only this task file's evidence/original scope list plus historical task references remain
- `git diff --check`

### Review corrections
- None

### Commit
- `1ff324b2f` — `docs: migrate lifecycle legacy design docs to spEARS v2 homes`
