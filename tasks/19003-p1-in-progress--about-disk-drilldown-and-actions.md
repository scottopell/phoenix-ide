# Make `/about` disk usage fast, drillable, and actionable

Supersedes `tasks/07004-p1-ready--about-disk-drilldown-actions.md` with a cache-busting slug change.

## Problem

`/about` now reports Phoenix-managed worktree disk usage, but the disk section still has two important product gaps:

1. Disk sizing is part of the general deployment-info request. Managed worktrees can contain large generated directories, so opening `/about` can be delayed by recursive sizing work even though build/network/resource facts are cheap.
2. Managed worktree usage is aggregated. If the aggregate is large, the user cannot tell which worktree is responsible or resolve the issue from the app.

Users deserve a diagnostic page that loads immediately, identifies the biggest Phoenix-created disk consumers, and offers safe actions to resolve them.

## Goal

Split disk sizing into its own API surface and make the Phoenix-managed worktree aggregate expandable into per-worktree rows with typed, safe actions:

- live/non-terminal worktrees link to the owning conversation;
- leftover worktrees can be cleaned up only after backend revalidation proves no live owner remains.

## Proposed design

### 1. Split deployment facts from disk sizing

- Keep `GET /api/deployment` fast for build, network/TLS, process/system resources, log sinks, local-access, and static paths that do not require expensive recursive walks.
- Add a dedicated disk endpoint, e.g. `GET /api/deployment/disk`, for all disk sizing.
- `/about` renders the general deployment page immediately and loads the disk section independently with its own loading/error/refresh state.
- Disk sizing should run off the async handler path using `spawn_blocking` or a dedicated cached sizing service.
- The disk response should include its own `sampled_at` timestamp so users can refresh only disk health.

### 2. Return typed disk categories and worktree details

Avoid relying on labels/path strings for semantics. Extend the disk response with typed categories/dispositions, for example:

```rust
DiskEntry {
    category: DiskCategory,
    label: String,
    path: String,
    size: DiskSize,
}

DiskCategory = Database | DataDirectory | ManagedWorktrees | PrContext | BrowserCache | BrowserProfiles | Tls | Skills | Credentials | Attachments
```

For managed worktrees, return a drilldown collection in addition to the aggregate:

```rust
ManagedWorktreeDiskEntry {
    path: String,
    size: DiskSize,
    disposition: ManagedWorktreeDisposition,
}

ManagedWorktreeDisposition =
    Live { conversation_id, title, state, archived }
  | Leftover { source_conversation_id, source_state, archived, cleanup_allowed }
```

The backend, not the UI, decides disposition. A terminal/archived row is not a live owner; an existing directory at a DB-known managed path with no live owner is a leftover.

### 3. Make the aggregate drillable

- The `Phoenix-managed worktrees` aggregate row expands inline.
- Expanded rows show each worktree's measured size, project/repo context where available, conversation title/id, state, and path.
- Sort per-worktree rows by measured size descending; not-measured/absent entries should be grouped predictably after measured entries.
- Preserve the current aggregate row and disk-health summary, but stop users from needing shell access to find the largest offender.

### 4. Add safe actions

For `Live` worktrees:

- Show **Open conversation**.
- Do not offer direct delete/cleanup from `/about`; work should flow through the conversation lifecycle actions that know how to preserve user work.

For `Leftover` worktrees:

- Show **Clean up leftover** / **Retry cleanup** only when backend disposition says cleanup is allowed.
- Add a mutation endpoint, e.g. `POST /api/deployment/disk/managed-worktrees/cleanup`, that revalidates before deleting anything.
- The cleanup endpoint must re-check:
  - path is one of the DB-known `cm_worktree_path` values;
  - path matches the strict Phoenix worktree shape `{repo}/.phoenix/worktrees/{id}`;
  - no live/non-terminal owner still owns that work scope;
  - directory still exists;
  - cleanup target matches persisted mode semantics: Work / managed Explore may remove Phoenix-created branch; Branch removes only the worktree and keeps the user branch;
  - operation is idempotent.
- After cleanup, refresh only the disk endpoint and report success/failure inline.

### 5. Specs and tests

- Update `specs/deployment-info/requirements.md` to cover separate disk loading, drilldown, and read/write boundary for cleanup actions.
- Consider a small Allium spec only for the cleanup mutation if the lifecycle/disposition preconditions become multi-step enough to warrant it.
- Add backend tests for:
  - live vs leftover disposition;
  - no cleanup action for live owners;
  - cleanup rejects non-DB-known paths;
  - cleanup rejects malformed/non-Phoenix paths;
  - cleanup is idempotent when a leftover directory is already gone;
  - Branch-mode cleanup preserves the branch.
- Add frontend tests for:
  - `/about` renders general deployment facts before disk sizing resolves;
  - disk section loading/error/refresh states;
  - aggregate expansion and size ordering;
  - live rows render Open conversation;
  - leftover rows render cleanup and refresh after success.

## Acceptance criteria

- Opening `/about` does not wait for recursive disk sizing.
- Disk health loads via a separate request with its own loading/error/refresh affordance.
- Managed worktree aggregate is expandable into per-worktree rows sorted by measured size.
- Users can open live worktree conversations from `/about`.
- Users can clean up leftover worktrees from `/about` only through a backend-rechecked, idempotent cleanup endpoint.
- The UI never infers cleanup eligibility from labels or path strings; it renders typed backend disposition.
- Cleanup never deletes a worktree still owned by a live conversation and never deletes a user branch for Branch-mode worktrees.
