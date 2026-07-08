# spEARS v2 Migration for Browser Viewer and Work-Scope Specs

## Problem

The stop-browser-session work crosses three related spec domains: `viewer_slot`, `browser-tool`, and `work-scope-ui`. Those specs currently have mixed levels of spEARS v2 maturity, and at least `work-scope-ui` still carries legacy `design.md` material. The implementation task should make only bounded spec edits for the new behavior; a broader migration needs its own focused pass.

## Goal

Run a full spEARS v2 migration/audit for the browser viewer and work-scope lifecycle surface, covering:

- `specs/viewer_slot/`
- `specs/browser-tool/`
- `specs/work-scope-ui/`
- shared ADRs under `specs/adrs/` where durable tradeoffs need preserving

Use the `spears-v2-migrate` skill to guide the work.

## Scope

- Read each domain as a whole: `requirements.md`, `executive.md`, any `.allium`, any legacy `design.md`, active tasks, and code/spec references that cite design docs as authority.
- Classify load-bearing content into the correct spEARS v2 home:
  - timeless user need / named requirements → `requirements.md`
  - precise lifecycle/state/invariant behavior → `.allium`
  - durable design decisions and tradeoffs → `specs/adrs/NNN_*.md`
  - status/current implementation/verification coverage → `executive.md`
  - implementation-local facts → code comments
  - stale rollout/checklist/file-layout notes → delete
- Retire legacy `design.md` files where their unique content has a v2 home or no living spec value.
- Remove or rewrite active references that point at retired design docs as current authority.
- Keep requirements and Allium timeless: no task IDs, rollout history, decision logs, or status-relative language.

## Expected output

- Updated v2 spec artifacts for the three domains.
- ADRs only where there were real durable decisions/tradeoffs worth preserving.
- Deleted `design.md` files where migration is complete; otherwise an explicit blocker and follow-up.
- Updated `specs/adrs/README.md` if new ADRs are added.
- A short handoff summary identifying what moved, what was deleted, and any blocked remainder.

## Validation

- Run targeted Allium checks for any changed `.allium` files.
- Run the spec pre-flight from `specs/AUTHORING.md`.
- Run at least `./dev.py check --lanes spec-shape,allium,spec-anchors,fast`; run full `./dev.py check` if code or broad repo guidance changes.

## Non-goals

- Do not implement new browser-session UI behavior in this task.
- Do not invent ADR rationale that is not supported by existing artifacts.
- Do not keep a legacy `design.md` as a comfort blanket once its load-bearing content has migrated.
