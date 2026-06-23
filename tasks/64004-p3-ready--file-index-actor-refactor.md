Refactor the file index from a Mutex-of-state model to a per-workspace actor that owns its own debouncer.

## Why this is a follow-up, not part of #217

PR #217 landed the in-memory cache with Loading/Ready state and an event-buffer to close the bootstrap publication race. Across three rounds of Codex review the pattern that kept surfacing was: shared mutable state (cells map + shared debouncer + per-workspace RwLock) coordinated by multiple locks, with each Codex finding pointing at "this composition isn't airtight at site X." The mid-level fix shipped in #217 closes the user-visible races but keeps the same coordination shape; each future feature that needs file-system state will add another lock interaction.

## Target shape

One actor per workspace, owning:
- Its own `notify_debouncer_full::Debouncer` (one inotify instance per workspace)
- Its `WorkspaceIndex` (paths, watched_dirs, gitignore)
- A command channel (`Search`, `Invalidate`, etc.)
- The notify event receiver for its own debouncer

The `WorkspaceIndexer` becomes a `HashMap<PathBuf, mpsc::UnboundedSender<WorkspaceCmd>>` + a spawn site for new actors.

## What this fixes by construction

- Bootstrap TOCTOU: events queue in the actor's mailbox while bootstrap is in flight; nothing is dropped.
- Subtree-walk race: same — events for the new subtree go into the mailbox, the actor processes them in order.
- Shared-watch ownership: each workspace has its own debouncer, so dropping the actor drops the watches with no coordination.
- Pathless rescan: per-workspace event stream means we know which actor lost events.
- Nested workspace event fan-out: the actor model doesn't need it — each workspace receives its own events from its own debouncer; the prefix-scan routing in the current code disappears.
- Orphaned bootstrap watches: actor lifecycle = watch lifecycle, no race to manage.

The `affected_workspaces`/`affected_workspace_roots`/`all_workspace_roots`/`dispatch_event`/`invalidate_workspace`'s "still-needed" computation all go away.

## When to do this

This refactor is gated on a feature pulling for it. Build it when one of these lands:

- **Code intelligence as Phoenix tools** (find references, list files matching pattern, etc.) — needs a structured backend, not a single-purpose Cmd+P cache.
- **Streaming search results** — actor can serve partial snapshots while the bootstrap walk is still running, fixing first-keystroke UX on cold cache.
- **Persistent index across deploys** — actor owns its lifecycle, can serialize/restore on shutdown/startup. Closes the cold-walk-after-every-deploy cost.
- **Multi-workspace search** — broadcast a search command across actors and merge.
- **Watch arbitrary files** (CI logs, etc.) — additional command type on the actor.

If none of those are imminent (~next quarter), the current Mutex+Loading/Ready model is fine. Codex's residual edge cases shipped with PR #217 have negligible real-world impact (microseconds-to-seconds TOCTOU window once per workspace per process; very narrow nested-workspace edge cases).

## Estimated cost

~1 full day of careful work. The lock-shaped tests need to be rewritten against the actor's command/response API. Likely needs its own Codex review pass — but the failure modes will be in actor lifecycle (cancellation, channel close, panic propagation) instead of in lock composition.

## Cross-references

- PR #217 (where this was discussed and deferred): `feat: cache the file-search walk per workspace`
- Module docstring in `crates/phoenix-ide/src/file_index.rs` references this task as the cleaner endpoint.
