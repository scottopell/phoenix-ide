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
- Live process + system resource usage (RSS, CPU, system memory, CPU count),
  cross-platform (macOS + Linux)
- On-disk locations with sizes for small owned artifacts, paths-only for large
  caches, and a stable row for the attachment store
- The log sink (a process-owned file path when configured, otherwise stdout) —
  path only, never contents
- A single-snapshot fetch with explicit refresh

**Owned by other specs:**
- `specs/api/` — the HTTP router, JSON-handler pattern, and the registration of
  `GET /api/deployment`
- `specs/auth/` — the password middleware that gates the endpoint
- `specs/conversation-ui/` — the settings dropdown and conversation-list chrome
  the page entry attaches to

**Explicitly out of scope:**
- Rendering log file contents (path/sink only)
- Live-streaming resource gauges or historical charts (snapshot + refresh only)
- Any control that mutates server state (the page is observational)

## Why It Exists

The operator of a Phoenix instance — frequently the same person running it on
their own machine — needs to confirm what is actually running and where its bytes
live without dropping to a shell to read env vars, `du` the data directory, or
`ps` the process. The running process already knows its build, its binding, its
TLS configuration, and its data layout, and can cheaply sample its own resource
usage. Surfacing those facts in one read-only page turns a multi-command
investigation into a single glance, while staying safe to open because it changes
nothing.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-DEPLOY-001:** Reach "About this deployment" from settings | Planned | New lazy route mounting `AboutDeploymentPage`; entry added to `SettingsDropdown`. Read-only — no mutating controls. |
| **REQ-DEPLOY-002:** Report build identity and uptime | Planned | `BuildInfo` from `env!("CARGO_PKG_VERSION")`, `env!("PHOENIX_GIT_SHA")` (`unknown` sentinel preserved), and `hot_restart` start-instant/start-wallclock accessors. |
| **REQ-DEPLOY-003:** Report network binding and TLS configuration | Planned | `NetworkInfo` from the captured `DeploymentConfig`; `TlsInfo` derived from the resolved `tls::ConfigSource`/`LoadedConfig`. Plain-HTTP stated when disabled. |
| **REQ-DEPLOY-004:** Report live process and system resource usage | Planned | `ResourceUsage` sampled via the `sysinfo` crate; per-metric `Option` → `null` for unavailable, never `0`. Cross-platform macOS + Linux. |
| **REQ-DEPLOY-005:** Report on-disk locations and their sizes | Planned | `DiskEntry[]` with a `DiskSize` tagged union (`measured` / `not_measured` / `absent` / `inline_db`). Small owned dirs recursed; large caches paths-only. Attachment store row is `inline_db` until file-based attachment storage is active. |
| **REQ-DEPLOY-006:** Surface the log location, never the contents | Planned | `LogInfo` reports `stdout` unless a deployment-owned log file is configured. A process-owned log-file path via env var is tracked as a follow-up (`tasks/`); the page does not claim launcher redirection paths it cannot own. |
| **REQ-DEPLOY-007:** Freshness of sampled values | Planned | `sampled_at` timestamp on every snapshot; the page refresh re-fetches `GET /api/deployment` rather than caching. |

## Behavioural Specification

No Allium spec accompanies this feature. It has no state machine, no lifecycle
with preconditions, and no multi-step operation with partial-failure ordering —
it is a read-only snapshot endpoint plus a render. Per AGENTS.md, spEARS alone is
sufficient here; the wire shape in `design.md` and the requirements above are the
normative contract.

## Cross-Spec Cross-References

- `specs/api/`: `GET /api/deployment` is registered in the same router and behind
  the same auth middleware as the other `/api/*` JSON endpoints.
- `specs/auth/`: the `/api/deployment` endpoint is gated by the password
  middleware and is not on the auth-exempt list. The `/about` SPA route is
  exempt (like other top-level SPA routes) so the shell loads on a
  password-protected hard refresh; only the static shell is exempt, not the data.
- `specs/conversation-ui/`: the "About this deployment" entry lives in the
  settings dropdown mounted in the conversation-list chrome.
