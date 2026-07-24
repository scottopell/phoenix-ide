# ADR-023: Projects accept taskmd files by default and plain markdown briefs through one task-source seam

- **Status:** Accepted
- **Date:** 2025-08-08
- **Affects:** REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012, REQ-PROJ-033, REQ-PROJ-034, REQ-PROJ-037

## Context

The projects workflow began with one task-file shape: a taskmd-style filename under the
project's tasks directory, with task metadata carried entirely in the filename. That shape
works well for repositories that already use taskmd, but Phoenix also needs to support
repositories that have not adopted taskmd and workflows where the brief already exists as a
plain markdown document elsewhere in the worktree.

The design choice is not just about input parsing. Approval, branch naming, review display,
status renaming, and fork spawning all depend on how Phoenix classifies the proposed file.
If the system modeled taskmd and plain briefs as unrelated flows, every approval path would
need separate logic and the review surface could drift across modes.

## Options considered

1. **Require taskmd everywhere.**
   - Pros: one file shape, one branch-naming rule, one approval path.
   - Cons: excludes repositories that do not use taskmd and forces agents to rewrite an
     existing brief into taskmd form before Phoenix can review it.

2. **Accept any markdown file but infer behavior ad hoc at each call site.**
   - Pros: flexible input.
   - Cons: classification rules would be duplicated across Explore approval, writing-mode
     fork proposals, and Request Changes promotion, making drift likely.

3. **Accept taskmd by default and plain markdown through one shared task-source seam.**
   - Pros: preserves taskmd as the ergonomic default while keeping one classification source
     of truth for every workflow that consumes a proposed brief.
   - Cons: approval and spawning logic must branch on task source and carry the plain-brief
     differences deliberately.

## Decision

Phoenix accepts two proposal shapes behind one shared task-source seam:

- **taskmd** remains the default. A taskmd-named file must live under the project's tasks
  directory, and its id, priority, status, and slug come from the filename.
- **plain markdown** means any other `.md` file inside the allowed root. It carries no
  structured task metadata, uses the first H1 or file stem for display title, defaults to
  priority `p2`, does not receive an approval-time status rename, and derives its branch name
  from the sanitized stem plus a conversation-derived uniquifier.

Every workflow that consumes a proposed brief uses this same classification:

- Explore `propose_task` review and approval
- writing-mode fork proposals
- Request Changes promotion into a fresh Explore conversation

This is one behavioral family with two file shapes, not two unrelated features.

## Consequences

- Repositories can use the managed workflow without adopting taskmd first.
- taskmd remains the preferred path where filename-encoded metadata and task-directory
  validation matter.
- Approval, fork spawning, and refinement must preserve the deliberate differences between
  taskmd and plain briefs instead of normalizing everything into taskmd prematurely.
- Future proposal backends can be introduced by extending the task-source seam, but no future
  backend is part of the current normative behavior.

## References

- `specs/projects/requirements.md` REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012, REQ-PROJ-033, REQ-PROJ-034, REQ-PROJ-037
- `specs/projects/projects.allium`
