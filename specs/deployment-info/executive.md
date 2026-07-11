# Deployment Info — Executive Summary

## Scope and Boundary

This spec governs the **"About this deployment" page** — a read-only diagnostic
view reachable from the conversation-list settings surface that answers "what
exactly is this running instance, where does it keep its data, and how much of
the machine is it using right now?"

**In scope:**
- Build identity: version, git SHA, uptime, start time
- Network binding and TLS posture (mode, cert/key/CA paths, auto-mode hosts,
  socket activation)
- A live managed-resource monitor with a focused resource endpoint, host and
  managed totals, per-category attribution, per-process rows, and bounded
  client-side recent history
- On-disk locations with sizes for small owned artifacts, paths-only for large
  caches, and a stable row for the attachment store
- The active log sinks (stdout and/or a process-owned file path) — path only,
  never contents
- Typed managed-worktree drilldown and backend-revalidated cleanup for leftover
  Phoenix-managed worktrees

**Owned by other specs:**
- `specs/api/` — the HTTP router, JSON-handler pattern, and registration of the
  deployment endpoints
- `specs/auth/` — the password middleware that gates the deployment data
  endpoints
- `specs/conversation-ui/` — the settings dropdown and conversation-list chrome
  the page entry attaches to
- `specs/process-inspector/` — the shared process-sampling primitives reused for
  managed bash attribution

**Explicitly out of scope:**
- Rendering log file contents (path/sink only)
- Durable server-side time-series retention or a general-purpose monitoring
  system
- Attribution for Browser, tmux/terminal, or MCP native processes until those
  subsystems surface process identity

## Why It Exists

The operator of a Phoenix instance — frequently the same person running it on
their own machine — needs to confirm what is actually running and where its bytes
live without dropping to a shell to read env vars, `du` the data directory, or
`ps` the process. The running process already knows its build, its binding, its
TLS configuration, and its data layout, and can sample both its host environment
and the native processes Phoenix manages. Surfacing those facts in one
read-only page turns a multi-command investigation into a single glance, while
staying safe to open because ordinary inspection changes nothing.

## Current Reality

| Requirement | Status | Notes |
|---|---|---|
| **REQ-DEPLOY-001:** Reach "About this deployment" from settings | Implemented | `ui/src/components/SettingsDropdown.tsx` links to the route rendered by `ui/src/pages/AboutDeploymentPage.tsx`. The page is read-only apart from typed leftover-worktree cleanup actions. |
| **REQ-DEPLOY-002:** Report build identity and uptime | Implemented | `crates/phoenix-ide/src/api/deployment.rs` builds `BuildInfo` from `env!("CARGO_PKG_VERSION")`, `env!("PHOENIX_GIT_SHA")`, and `crate::hot_restart::{started_at, uptime_secs}`. The UI renders version, git SHA, started time, and uptime. |
| **REQ-DEPLOY-003:** Report network binding and TLS configuration | Implemented | `DeploymentInfo.network` carries bind address, socket activation, and `TlsInfo`; `AboutDeploymentPage` renders plain HTTP explicitly when TLS is disabled and shows cert/key/CA/hosts when enabled. |
| **REQ-DEPLOY-004:** Report live managed-resource and host usage | Implemented | `GET /api/about/resources` returns `AboutResourcesSnapshot` with host metrics (`logical_cpu_count`, memory totals, load averages, busy/idle CPU fields, and system CPU where available) plus managed totals and category rows. API and Bash are attributed by PID; Browser, tmux/terminal, and MCP are explicit unavailable categories with reasons. Managed totals include both `process_count` and `deduplicated_pid_count`. |
| **REQ-DEPLOY-004A:** Poll live resource data while the page is visible | Implemented | `ui/src/pages/AboutDeploymentPage.tsx` polls with `RESOURCE_POLL_MS = 1_000`, skips both the initial fetch and timed refreshes while `document.visibilityState !== 'visible'`, triggers an immediate fetch on `visibilitychange` back to visible, guards overlap with `resourcesInFlightRef`, and ignores completions from unmounted or obsolete effects. |
| **REQ-DEPLOY-004B:** Maintain bounded rolling history and rollups | Implemented | The UI keeps up to five minutes of good samples via `appendResourceHistory` and `RESOURCE_HISTORY_RETENTION_MS = 5 * 60 * 1_000`, then derives current/average/peak CPU and memory rollups with `computeResourceRollups`. Charts and summary cards read from that bounded client-side history. |
| **REQ-DEPLOY-004C:** Preserve last-good semantics across refresh failures | Implemented | On fetch failure, `fetchResources` leaves `sample` and `history` intact, marks `stale: true` when a prior sample exists, and surfaces the error as `Live data stale — …`. When no sample exists yet, the page shows the error without fabricating data. |
| **REQ-DEPLOY-004D:** Keep deployment facts and live resource monitoring separate | Implemented | `GET /api/deployment` carries build, network, log, and locality facts without a parallel resource snapshot; `GET /api/about/resources` is the sole live CPU, memory, load, and managed-process telemetry contract; and `GET /api/deployment/disk` owns explicit disk sizing. The page refresh buttons can refresh resources or disk independently. |
| **REQ-DEPLOY-005:** Report on-disk locations and their sizes | Implemented | `GET /api/deployment/disk` returns `DeploymentDiskInfo` with typed `DiskSize` variants, aggregate managed-worktree sizing, and per-worktree rows with typed disposition. The UI keeps disk loading/error state separate from the live resource monitor. |
| **REQ-DEPLOY-006:** Surface the log sinks, never the contents | Implemented | `LogInfo` reports independent stdout/file sinks from the logger configuration, and the page renders sink facts only — never log contents. |
| **REQ-DEPLOY-007:** Freshness of sampled values | Implemented | `DeploymentInfo`, `DeploymentDiskInfo`, and `AboutResourcesSnapshot` all carry `sampled_at`; the UI distinguishes general refresh, disk refresh, and continuous resource sampling rather than serving one cached omnibus snapshot. |
| **REQ-DEPLOY-008:** Safely clean up leftover managed worktrees | Implemented | `POST /api/deployment/disk/managed-worktrees/cleanup` revalidates DB ownership, Phoenix path shape, live-conversation ownership, and mode semantics before removal; the UI offers cleanup only for typed leftover rows whose disposition allows it. |

## Behavioural Specification

No Allium spec accompanies this feature. The page composes read-only endpoint
fetches plus a narrowly scoped cleanup action, but it does not introduce a new
state machine or lifecycle whose invariants would benefit from an Allium layer.
The normative artifacts here are `requirements.md` plus the Rust/TS wire types
exported from `crates/phoenix-ide/src/api/deployment.rs` into `ui/src/generated/`.

## Cross-Spec Cross-References

- `specs/api/`: the deployment endpoints are registered in the shared router:
  `GET /api/deployment`, `GET /api/about/resources`, `GET /api/deployment/disk`,
  and `POST /api/deployment/disk/managed-worktrees/cleanup`.
- `specs/auth/`: the deployment data endpoints are behind the password
  middleware; the SPA route can load the shell, but data fetches remain gated.
- `specs/conversation-ui/`: the "About this deployment" entry lives in the
  settings dropdown mounted in the conversation-list chrome.
- `specs/process-inspector/`: bash resource attribution reuses the shared
  process-sampling machinery for CPU/process-count/PSS-style memory sampling.
