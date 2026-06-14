# Prune .phoenix/pr-context/ bundles to bound disk growth

PR auto-fix context bundles are written to `.phoenix/pr-context/pr-{N}-{timestamp}.json`
in each conversation worktree on every "Address PR feedback & CI" click. Each click
writes a NEW file (timestamp in name) and nothing ever removes them, so they grow
unbounded — one conversation already accumulated three.

They are also invisible to the user: the `/about` deployment-info page
(`AboutDeploymentPage` -> `GET /api/deployment` -> `build_disk_locations`) only
reports top-level Phoenix paths (db, data-dir aggregate, TLS, skills, browser cache).
It never enumerates per-worktree `.phoenix/` contents, so these bundles are
phoenix-owned artifacts on disk that no user can see or manage.

Fix: prune at capture time — keep the last N (e.g. 3) bundles per PR number in a
worktree, deleting older ones when writing a new capture. Capture site is
`capture_pr_auto_fix_context_for_pr_item` in `crates/phoenix-ide/src/api/pr_monitoring.rs`
(writes under `worktree/.phoenix/pr-context/`).

Do NOT surface them on the /about page — retention is the right lever, not visibility.
