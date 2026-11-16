# Ember phase 2: retire the stale delivery roadmap

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). User explicitly requested deprecation because the roadmap is stale and infrequently used. This is a standalone retirement slice; scanner delivery is not a dependency.

## Outcome

New agents stop treating #806 as current coordination authority or posting routine records. No automation rewrites the historical Issue. Historical evidence remains accessible.

## Repository work

- New ADR supersedes ADR-069 and records the user's decision. Deprecate REQ-ROADMAP-001–014 explicitly while retaining IDs/original text. Mark the ENTIRE retained protocol historical and non-normative; no original SHALL remains an active obligation. Merely marking headings deprecated is insufficient. Update specs/roadmap/executive.md to the true retirement state.
- Replace mandatory reading/posting guidance in AGENTS.md and canonical skills/phoenix-development/SKILL.md. CLAUDE.md imports AGENTS.md and .agents/skills aliases canonical skills; do not edit aliases as separate authorities.
- Remove .github/workflows/roadmap-issue-reducer.yml, scripts/roadmap-issue-reducer.mjs and its dedicated tests, and their CI invocation in .github/workflows/ci.yml. Explicitly explain test retirement with feature removal. Preserve unrelated CI/task validation.
- Reconcile active roadmap duties in tasks 17001, 44013, 45015, 06002, 98020, 46005, 33001, 68002 and 58059. Preserve quoted historical commissions, incident evidence and unrelated scope/status.
- Existing authorities remain specs/Allium, ADRs, task filenames, PRs, deployed evidence and current user instructions. Tracking is not a replacement authority for their status.

## Live cutoff

Closing #806 alone does not stop reducer run(): it checks configured coordinator plus activation marker and never Issue state. A run already past its guard can overwrite an archive banner.

Inspect workflow/runs/config/body; disable and quiesce updater executions; clear PHOENIX_ROADMAP_ISSUE_NUMBER and PHOENIX_ROADMAP_COORDINATOR_HARNESS; remove activation marker and add a historical-only notice while retaining the final projection/comments; verify stable body and no running updater. Remove the source workflow through normal integration. Issue closure is optional and not required for correctness. Never delete Issue/comments.

These are concrete live GitHub changes to execute during the retirement slice with fresh destination evidence. Do not claim them done from a branch diff. Do not post a new roadmap protocol record merely to maintain the system being retired.

## Data transfer

No bulk import and no dual-write period. Reconcile still-live obligations selectively against current tasks/owners/PRs/user instructions, retaining historical links as evidence. The usable tracker is a destination when available; it does not gate stopping obsolete automation. Old holds are neither resurrected nor revoked by copying or dropping a stale row.

## Done when

No executable updater path or active posting instruction remains; workflows/CI reference no removed scripts; REQ IDs/history remain retrievable; live workflow/config/Issue readback proves cutoff; only confirmed current obligations are carried forward. Use git diff --check, task validation, workflow checks and scoped reference search. Rust/UI builds are unnecessary solely for this retirement.

Out of scope: deleting historical evidence, blanket roadmap import, dual writing and waiting for the scanner to stop stale automation.

## Implementation and live cutoff — 2026-11-16

Source retirement removes the updater workflow/reducer and its dedicated tests/CI
step together. ADR-262 supersedes ADR-069; the original protocol remains verbatim
beneath an explicit whole-document non-normative archive notice, with all 14 IDs
marked retired in the executive. Canonical agent guidance and the named active
task duties no longer require reading/posting to the roadmap. Historical
commissions, evidence and unrelated scope remain.

Live workflow330648486 is `disabled_manually`; both repository variables were
deleted. No in-progress/waiting updater jobs were present. Issue806 now has a
historical-only notice and no activation marker; exact body readback and a later
independent read were stable. The parent integrator separately verified workflow,
variables, notice and absent marker. Two stale November11 records still report
queued but contain zero jobs; GitHub refuses both normal and force cancellation
with409 “has not been queued yet.” Do not claim zero queued records. The disabled
workflow, absent configuration and absent marker prevent writes if these later
start. Final projection/comments are retained; no roadmap record was posted.

Validation: task/spec-shape/spec-anchors lanes pass; CI YAML parses; exact CI diff
removes only the retired reducer step; original protocol text and all14 retained
IDs verified; added relative links and whitespace pass. Rust/UI checks are not
applicable. Source PR review/CI and merge remain integrator-owned gates before
marking this task done.
