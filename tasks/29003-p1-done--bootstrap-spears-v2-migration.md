# Bootstrap Phoenix onto spEARS v2

## Context

spEARS v2 changes the artifact model in ways that affect both Phoenix-the-product and Phoenix-the-codebase:

- `design.md` is no longer a current-design artifact.
- precise current behavior belongs in `.allium` only when complexity warrants it.
- design rationale moves to a single shared project-level ADR chain at `specs/adrs/`.
- `requirements.md` and `.allium` are normative; ADRs are authoritative history; `executive.md` is status/current reality.
- the built-in Phoenix `spears` skill still teaches the v1 three-document model.
- Phoenix’s own repo guidance and ~49 specs still assume `requirements.md` + `design.md` + `executive.md`.

A read-only copy of the new v2 source was provided out-of-tree during task execution.

## Goal

Create the first safe migration slice: make Phoenix teach and follow spEARS v2 for new work, establish the ADR-chain foundation, and produce a concrete follow-up plan for migrating existing v1 specs without attempting a risky repo-wide rewrite in one task.

## Scope

1. **Update the built-in product skill**
   - Replace `crates/phoenix-skills/src/builtin/spears/` with the provided v2 skill content.
   - Preserve Phoenix packaging expectations for built-in skills and companion files.
   - Update any unit tests that assert v1 reference filenames such as `references/discover.md`.

2. **Update repo guidance for new work**
   - Update `AGENTS.md` spEARS/specification guidance to match v2:
     - no required `design.md` for new specs,
     - shared `specs/adrs/` chain for design decisions,
     - `.allium` as precision-on-demand,
     - artifact-aware timelessness rules,
     - `executive.md` remains the status/current-reality exception.
   - Avoid rewriting unrelated workflow guidance.

3. **Bootstrap Phoenix’s ADR chain**
   - Add `specs/adrs/_TEMPLATE.md` and `specs/adrs/README.md` based on v2.
   - Add an initial Phoenix ADR that records the methodology migration decision itself, scoped to `methodology-level`.
   - Keep ADR prose honest: point-in-time, with options considered and negative consequences.

4. **Inventory existing v1 spec assumptions**
   - Produce a concise migration inventory for the existing ~49 `specs/*/design.md` files:
     - specs that already have `.allium` and can eventually retire/reduce design prose,
     - specs without `.allium` that need either no precise layer or a future Allium distillation,
     - high-risk `design.md` files referenced by code comments/tasks and therefore needing careful ADR/Allium extraction.
   - Do not delete or mass-rewrite existing `design.md` files in this task.

5. **Create follow-up tasks**
   - Spin out concrete tasks for the next migration phases, likely including:
     - deterministic validation/script updates for v2 spec shape,
     - high-priority design-doc decomposition for core specs such as `bedrock`, `projects`, `bash`, `terminal`, and `mcp`,
     - code-comment/reference cleanup where comments cite `design.md` for current behavior,
     - any Allium distillation/weed work discovered during inventory.

## Non-goals

- Do not migrate all existing Phoenix specs to v2 in one pass.
- Do not delete existing `design.md` files until their current-behavior and design-rationale content has a new home.
- Do not invent ADR rationale where the decision history cannot be reconstructed.
- Do not change Phoenix runtime/product behavior beyond the built-in skill content.

## Validation

- Run the relevant Phoenix checks for built-in skills and formatting; use `./dev.py check` if feasible.
- Confirm the built-in `spears` skill extracts with the new reference/adrs companion files.
- Confirm no remaining built-in skill text teaches the v1 three-document model as current guidance.
- Confirm new ADR files are discoverable under `specs/adrs/` and follow the v2 template.

## Expected follow-up shape

This task should end with Phoenix on spEARS v2 for new work, plus a sequenced migration map. Existing `design.md` files remain as legacy artifacts until follow-up tasks migrate each domain’s content into requirements, Allium, ADRs, or executive status as appropriate.

## Notes from initial exploration

- v2 source files found in the provided out-of-tree source copy:
  - `skills/spears/SKILL.md`
  - `skills/spears/references/*.md`
  - `skills/spears/adrs/*.md`
- Phoenix built-in skill currently lives at `crates/phoenix-skills/src/builtin/spears/` and has v1 references: `discover.md`, `ears-format.md`, `implement.md`, `lint.md`, `reflect.md`, `validate.md`.
- Phoenix currently has roughly 49 `design.md` files, 49 `requirements.md` files, 53 `executive.md` files, and 37 `.allium` files under `specs/`.
- Rust comments and module docs still cite several `specs/*/design.md` files as authoritative design/current-behavior references; these should be handled in focused follow-ups.

## Acceptance criteria

- Built-in `spears` skill content matches spEARS v2 structure and routing.
- Repo-level guidance no longer instructs agents to create or maintain v1 `design.md` as the current design artifact.
- `specs/adrs/` exists with template, index, and an initial methodology ADR.
- A migration inventory and follow-up task set exists for legacy Phoenix specs.
- Existing legacy specs are left intact unless deliberately touched for bootstrapping references.
- Checks relevant to touched code/docs pass or any failures are documented with next steps.

## Migration inventory

This bootstrap leaves all legacy `specs/*/design.md` files in place. The current tree has 49 legacy design docs:

### Legacy design docs with Allium already present

These specs already have a precise behavior layer that can absorb or replace current-behavior prose during later decomposition:

- `agents` → `agents.allium`
- `auth` → `auth.allium`
- `bash` → `bash.allium`
- `bedrock` → `bedrock.allium`
- `browser-tool` → `browser-lifecycle.allium`, `browser-profiling.allium`
- `builtin-skills` → `builtin-skills.allium`
- `command-suggestion` → `command-suggestion.allium`
- `credential-helper` → `credential-helper.allium`
- `inline-references` → `inline-references.allium`
- `llm` → `anthropic.allium`, `llm.allium`, `responses.allium`
- `llm-retry-visibility` → `llm-retry-visibility.allium`
- `mcp` → `mcp.allium`
- `messagelist-render-units` → `render_units.allium`
- `notifications` → `notifications.allium`
- `patch` → `patch.allium`
- `permissions` → `permissions.allium`
- `pr-association` → `pr-association.allium`
- `projects` → `projects.allium`
- `skills` → `skills.allium`
- `stale-tool-results` → `stale-tool-results.allium`
- `subagents` → `subagents.allium`
- `terminal` → `terminal.allium`
- `terminal-panel` → `terminal-panel.allium`
- `tmux-integration` → `tmux-integration.allium`
- `wake-contracts` → `wake-contracts.allium`
- `work-actions-bar` → `work-actions-bar.allium`
- `work-lifecycle` → `work-lifecycle.allium`
- `working-phase-visibility` → `working-phase-visibility.allium`

### Legacy design docs without Allium

These specs need triage: many may be simple enough to retire design prose into requirements/executive/ADRs without Allium, while lifecycle or stateful areas need future Allium distillation.

- `agent-identity`
- `analytics`
- `api`
- `ask-user-question`
- `chains`
- `command-palette`
- `commission-review`
- `conversation-retrieval`
- `conversation-ui`
- `deployment-info`
- `file-explorer`
- `keyboard-interaction`
- `keyword_search`
- `process-inspector`
- `prose-feedback`
- `simple_client`
- `skills-ui`
- `tasks-ui`
- `think`
- `voice-input`
- `work-scope-ui`

### High-risk legacy design references

Code and generated docs still cite `design.md` as an authority in several domains. These should be decomposed before deleting the corresponding design docs:

- `bedrock`: Rust handlers and DB comments cite FM-7 and context-continuation design prose.
- `projects`: `projects.allium` and historical tasks cite cross-domain design rules.
- `bash`: tool implementation comments cite design sections for process, watch-channel, and output behavior.
- `terminal` / `terminal-panel` / `tmux-integration`: terminal crates, generated wire types, and Allium comments cite design sections for auth, attach, HUD, tmux output, and environment handling.
- `mcp`: tasks and design references describe the native HTTP transport boundary.
- `chains`: backend, UI utilities, tests, and generated types cite sidebar grouping and work identity design prose.
- `working-phase-visibility`, `wake-contracts`, `llm`, `stale-tool-results`, and `messagelist-render-units`: Allium files cite design prose for carrier fields, semantic exceptions, or unresolved confirmations.
- Specs such as `process-inspector`, `work-scope-ui`, `deployment-info`, and `commission-review` cite their design docs for non-goals or exact wire/behavioral rationale despite lacking Allium.

## Follow-up tasks created

- `tasks/29004-p1-ready--spears-v2-validation.md` — update deterministic validation and authoring preflight for spEARS v2 shape.
- `tasks/29005-p1-ready--decompose-core-design-docs.md` — decompose high-risk core design docs (`bedrock`, `projects`, `bash`, `terminal`, `mcp`).
- `tasks/29006-p2-ready--cleanup-design-md-code-references.md` — replace code/generated/task-template design-doc authority citations with REQ IDs, Allium entities, symbols, or ADR links.
- `tasks/29007-p2-ready--triage-non-allium-design-docs.md` — decide which non-Allium legacy specs need distillation versus lightweight retirement.
