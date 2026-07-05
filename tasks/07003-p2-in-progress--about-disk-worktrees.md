# Add Phoenix-managed worktrees to `/about` disk usage health

## Problem

The `/about` page's "On disk" report lists Phoenix's database, data dir, browser/cache locations, credentials, and PR context bundles, but it does not show the Phoenix-managed git worktrees under each repo's `.phoenix/worktrees/`. Those worktrees are often the largest Phoenix-owned disk consumer, so the current page can look healthy while most Phoenix-created bytes are hidden.

## Goal

Make `/about` a trustworthy disk-pressure diagnostic by surfacing Phoenix-managed worktree disk usage and summarizing disk-health risk in a way that is honest about measured vs. unmeasured bytes.

## Plan

1. **Backend: enumerate DB-known Phoenix-created worktree paths**
   - Add a DB query/helper that returns distinct non-empty `cm_worktree_path` values across conversations, independent of conversation state/archive status.
   - This is not an ownership/liveness query. Normal terminal/archive cleanup removes managed worktrees; if an archived or terminal row still points at a directory that exists, the `/about` disk report should count it as Phoenix-created bytes left behind by a cleanup failure or stale deployment state.
   - Do not use only `get_work_conversations()`: it excludes archived rows and Work/Branch-only rows may miss managed Explore worktrees.
   - Keep the DB `cm_worktree_path` column as the source of truth for Phoenix-created paths; do not infer ownership by scanning arbitrary `.phoenix/worktrees` directories.

2. **Backend: add a `/api/deployment` disk row for managed worktrees**
   - Add a per-request aggregate `DiskEntry` labelled something like `Phoenix-managed worktrees`.
   - For each distinct managed worktree path:
     - If the directory exists, measure it with the existing symlink-safe directory-size walk (or a shared equivalent).
     - If no managed worktree directory exists, return `DiskSize::Absent` rather than `0`.
     - If enumeration fails, return `DiskSize::NotMeasured` and log at debug level, matching the existing PR-context aggregate pattern.
   - Display an honest path: when all worktrees belong to one repo root, use `<repo>/.phoenix/worktrees/*`; when multiple roots exist, show the first root plus a `(+N more roots)` suffix, or a relative pattern fallback when no root can be determined.
   - Deduplicate paths before measuring so continuation chains/sub-agents that share a worktree do not double-count.

3. **Frontend: improve the disk usage health readout**
   - Keep the current detailed table, including Reveal where the path is a concrete local path.
   - Add a compact summary above the table, derived from typed `DiskSize` values:
     - total measured bytes across rows,
     - count of measured / not-measured / absent entries,
     - a clear warning when significant disk categories are not measured,
     - highlight the managed-worktrees row when it is the largest measured category.
   - Avoid parsing path strings for semantics where possible; prefer label/category information from the typed response if the backend wire shape is extended.

4. **Specs and generated types**
   - Update `specs/deployment-info/requirements.md` so REQ-DEPLOY-005 explicitly includes Phoenix-managed worktrees as an on-disk location class.
   - Update the deployment-info design/current-reality docs only as needed, keeping them timeless and avoiding status/changelog language outside `executive.md`.
   - If the wire shape changes, run `./dev.py codegen` and commit regenerated `ui/src/generated/*` files.

5. **Tests**
   - Backend unit tests for worktree aggregation:
     - deduplicates shared worktree paths,
     - sums existing worktree directories,
     - reports absent when no managed worktree directories exist,
     - returns not-measured on DB enumeration failure if that path is testable.
   - Frontend tests for disk summary rendering:
     - total measured bytes,
     - unmeasured/absent counts,
     - largest-category highlighting for managed worktrees.
   - Run the relevant targeted tests, then `./dev.py check`.

## Acceptance criteria

- `/about` includes a visible `Phoenix-managed worktrees` disk row.
- The row measures existing Phoenix-managed worktree directories and does not double-count shared worktrees.
- Archived/terminal rows do not imply live ownership, but if their DB-known managed worktree paths still exist on disk, those bytes are included and thereby surface cleanup failures/stale deployment leftovers.
- The disk section gives a quick health summary instead of requiring the operator to manually inspect every table row.
- Missing or unmeasurable values are represented explicitly as `absent` / `not measured`, never as misleading zeroes.
- The page remains read-only; no cleanup/delete action is introduced in this task.
