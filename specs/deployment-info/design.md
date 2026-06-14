# Deployment Info — Design

## Overview

`GET /api/deployment` returns a single `DeploymentInfo` snapshot. The page
`AboutDeploymentPage` fetches it once on mount, renders it as grouped read-only
sections, and re-fetches on an explicit refresh. There is no streaming and no
mutation endpoint.

The snapshot has two kinds of data:

- **Static facts** resolved once at process startup (build identity, network
  binding, TLS configuration, the data directory layout). These are captured into
  a `DeploymentConfig` value in `main` and threaded into `AppState`, so the
  handler reads them from owned state rather than re-deriving environment
  variables on each request.
- **Sampled facts** measured per request (resource usage, on-disk sizes, the
  `sampled_at` timestamp). These are computed inside the handler at request time
  so a refresh yields current values (REQ-DEPLOY-007).

## Wire Shape

The response is typed on the Rust side and exported to `ui/src/generated/` via
`ts_rs`, so the TypeScript shape cannot drift from the Rust source. All field
names below are the JSON field names.

```
DeploymentInfo {
  build:      BuildInfo
  network:    NetworkInfo
  resources:  ResourceUsage
  disk:       DiskEntry[]
  log:        LogInfo
  sampled_at: string            // RFC3339 (DateTime<Utc>)
}

BuildInfo {
  version:        string         // env!("CARGO_PKG_VERSION")
  git_sha:        string         // env!("PHOENIX_GIT_SHA"), "unknown" if absent
  started_at:     string | null  // RFC3339; null if the start time was not recorded
  uptime_seconds: number         // u64
}

NetworkInfo {
  bind_address:     string       // e.g. "0.0.0.0:8000"
  socket_activated: boolean
  tls:              TlsInfo
}

TlsInfo {
  enabled:      boolean
  mode:         "auto" | "manual" | null   // null when disabled
  cert_path:    string | null
  key_path:     string | null
  ca_cert_path: string | null
  hosts:        string[]                    // auto mode host list; empty otherwise
}

ResourceUsage {                  // every field null when unsamplable on the host
  process_memory_bytes:           number | null   // RSS
  process_cpu_percent:            number | null
  system_total_memory_bytes:      number | null
  system_available_memory_bytes:  number | null
  logical_cpu_count:              number | null
}

DiskEntry {
  label: string                  // "Database", "Data directory", "TLS", ...
  path:  string                  // absolute path
  size:  DiskSize                // tagged union, below
}

DiskSize =                       // serde-tagged, ts-rs discriminated union
  | { kind: "measured", bytes: number }
  | { kind: "not_measured" }     // large dir, intentionally not walked
  | { kind: "absent" }           // path does not exist
  | { kind: "inline_db" }        // attachment store: bytes live inside the DB

LogInfo {                        // independent sinks; both may be active
  stdout: boolean                // logs written to stdout (supervisor-captured)
  file:   string | null          // absolute path of the process-owned log file
}
```

`DiskSize` is an enum, not a `size: number | null`, because the four states are
semantically distinct and a bare nullable number cannot tell "not measured"
apart from "absent" apart from "inline in the DB." Modelling them as a tagged
union makes the invalid combinations unrepresentable and forces the UI to render
each state deliberately (correct-by-construction; see AGENTS.md).

`ResourceUsage` fields are individually `Option` because availability is
per-metric, not all-or-nothing — a host may expose system memory but not
per-process CPU. `null` is the explicit "unavailable" marker REQ-DEPLOY-004
requires; the UI renders it as "unavailable," never as `0`.

## Backend

### Config capture (`main`)

`DeploymentConfig` is assembled in `main` and passed into `AppState` as a new
field. It holds the static facts:

- `bind_address: SocketAddr` — the address the server is actually bound to.
  Because socket activation hands the process a systemd-owned socket (often with
  `PHOENIX_PORT` unset), the listener is acquired *before* the config is built
  and `bind_address` is taken from `listener.local_addr()`, not the
  `0.0.0.0:PORT` default. Socket-activation status itself is read live in the
  handler via `hot_restart::is_socket_activated()` rather than captured.
- `tls: TlsInfo` — derived from the resolved `tls::ConfigSource`/`LoadedConfig`:
  disabled when the source is `None`; otherwise `mode`, `cert_path`, `key_path`,
  `ca_cert_path` from `LoadedConfig`, and `hosts` from `ConfigSource::Auto`.
- `log: LogInfo` — the active log sinks, derived from the same `LogConfig` that
  builds the subscriber (REQ-DEPLOY-006). `LogConfig` is resolved once from
  `PHOENIX_LOG_STDOUT` (bool, default on) and `PHOENIX_LOG_FILE` (optional path);
  both sinks are independent and may be active together.
- `locations: Vec<DiskLocation>` — the static on-disk layout. Each `DiskLocation`
  carries a `label`, an absolute `path`, and a `MeasureMode` dictating how it is
  sized at request time. The rows are: the database file (`File`); the data
  directory — recursed (`RecurseSmall`) only when it is a Phoenix-owned dedicated
  directory (`.phoenix-ide` for user installs and dev worktrees, `phoenix-ide`
  for the native `/var/lib/phoenix-ide` production root), otherwise reported
  path-only (`NoMeasure`) so a custom `PHOENIX_DB_PATH` cannot make a request walk
  `/tmp` or `$HOME`; the TLS inputs — the managed directory in auto mode
  (`RecurseSmall`) or the explicit certificate and key files in manual mode
  (`File` each); the built-in skills directory (`RecurseSmall`); the attachment
  store (`InlineDb` while attachments live in the database); the browser binary
  cache (`NoMeasure`); and the per-scope browser profile glob (`Pattern`). The
  active codex credential file is *not* a static row — it is resolved per request
  in the handler because the credential source can change at runtime (see below).
  Every path is absolute, resolved from the same logic the rest of the process
  uses, so the page reports the locations the process actually uses.

To avoid generating the auto cert twice, `tls::load_config` is called once in
`main`; its `LoadedConfig` feeds both the `TlsInfo` capture and the
`serve_https` call. The `ServerConfig` is carried forward to the already-bound
listener.

`MeasureMode` is the sizing policy per location: `File` stats one file;
`RecurseSmall` walks a small owned directory (never following symlinks, so a
symlink cycle or link to a large external tree cannot unbound the walk);
`NoMeasure` reports a known-large real directory as a path with
`DiskSize::not_measured` (or `absent` when missing); `Pattern` reports a
glob/pattern location as `not_measured` unconditionally (its literal path never
exists on disk); `InlineDb` reports the attachment store as `inline_db`, giving
file-based attachments a stable home in the report before that storage mode is
active (REQ-DEPLOY-005).

### Handler (`api`)

A `deployment_info` handler reads `state.deployment` (the captured
`DeploymentConfig`), samples the live values, and returns `Json(DeploymentInfo)`.
It is registered as `GET /api/deployment` behind the same auth middleware as the
other `/api/*` routes.

Sampling steps:

1. **Build:** `version`/`git_sha` from `env!`; `started_at` and `uptime_seconds`
   from `hot_restart`. `hot_restart` exposes the process start `Instant` and the
   start wall-clock `DateTime<Utc>` through public accessors so the handler can
   report both uptime and an absolute start time.
2. **Resources:** via the `sysinfo` crate, refreshing process and global
   memory plus the CPU list. Each metric is mapped to `Some(_)` when sysinfo
   provides it and `None` when it does not. The logical CPU count is the length
   of sysinfo's CPU list — the host total this field labels — rather than
   `std::thread::available_parallelism()`, which reflects the process's CPU
   affinity/quota and would under-report under a cgroup limit. sysinfo is the
   cross-platform sampler that satisfies the macOS + Linux requirement without
   per-OS `/proc` scraping.
3. **Disk:** one `DiskEntry` per static `DiskLocation`, sized per its
   `MeasureMode` (see Config capture). `File`/`RecurseSmall` produce
   `DiskSize::measured`; `NoMeasure`/`Pattern` produce `DiskSize::not_measured`;
   a missing real path yields `DiskSize::absent`; the attachment store yields
   `inline_db`. The handler then appends two rows resolved per request rather
   than at startup. The active codex credential row is resolved via
   `resolve_active_auth_path` (Phoenix's own `~/.phoenix-ide/codex-auth.json`, or
   Codex CLI's `~/.codex/auth.json` under `OPENAI_USE_CODEX_AUTH` piggyback mode,
   falling back to the canonical Phoenix path reported absent); resolving per
   request keeps the row correct after the in-app login flow switches the active
   credential source at runtime. The PR auto-fix context row aggregates the
   per-worktree `{worktree}/.phoenix/pr-context/` bundle directories: their parent
   worktrees live under each project's `{repo_root}/.phoenix/worktrees/`, so there
   is no single startup-known path to size. The handler enumerates Work/Branch
   worktrees from the database and sums each bundle directory's bytes into one
   `measured` row (`absent` when no worktree holds a bundle directory). The `path`
   is the `…/.phoenix/worktrees/*/.phoenix/pr-context` glob anchored at the lone
   project root when all bundles share one, and a root-relative glob otherwise,
   since a single `path` string cannot honestly point at several roots. Resolving
   per request reflects worktrees created and torn down after startup. Each bundle
   directory is capacity-bounded by the capture-site retention, so the aggregate
   walk stays cheap.
4. **`sampled_at`:** `Utc::now()` at the moment the snapshot is assembled.

The `dir_size` helper is bounded: it recurses only the directories the spec
classifies as small — the owned data directory, TLS and skills directories, and
the per-worktree PR-context bundle directories — and never follows into the
large-cache paths, so a single request cannot trigger a multi-gigabyte walk
(REQ-DEPLOY-005).

## Frontend

- **Route:** a new lazy route (e.g. `/about`) in `App.tsx` mounting
  `AboutDeploymentPage`, following the existing lazy-import pattern.
- **Entry point:** `SettingsDropdown` gains an "About this deployment" item that
  navigates to the route and closes the dropdown. The dropdown stays the home of
  toggles; the page is the home of the report (REQ-DEPLOY-001).
- **Page:** `AboutDeploymentPage` calls `api.deploymentInfo()` on mount, holds
  loading/error/data state, and renders grouped sections — Build, Network &
  TLS, Resources, Disk, Logs — using the existing `.view-header` and
  `.settings-section` classes. A refresh control re-invokes the fetch
  (REQ-DEPLOY-007). The `sampled_at` time is shown so the snapshot is visibly
  point-in-time.
- **Rendering rules:** `DiskSize` is matched exhaustively — `measured` shows a
  human-readable size, `not_measured` shows "not measured," `absent` shows
  "absent," `inline_db` shows "stored in database." `null` resource values show
  "unavailable." TLS-disabled renders "Serving plain HTTP." The log section
  renders one row per sink: stdout on/off, and the file path (or "none").
- **API:** `api.deploymentInfo()` is a plain `GET /api/deployment` returning the
  `ts_rs`-generated `DeploymentInfo` type imported from `ui/src/generated/`.

## Design Decisions

- **Static facts are threaded through `AppState`, not re-read from env in the
  handler.** The process resolves its paths and binding once at startup; a
  handler that re-reads `PHOENIX_DB_PATH` could disagree with what the process
  actually opened if the environment differs. Capturing the resolved values in
  `DeploymentConfig` makes the report authoritative by construction.
- **`sysinfo` over `/proc`.** The same binary runs on macOS and Linux;
  `/proc`-based sampling would silently report nothing on macOS. `sysinfo`
  abstracts both, and per-metric `Option` preserves honest unavailability.
- **Large caches are paths, not sizes.** Walking the browser binary cache or
  per-scope profiles on every page load would make a diagnostic page expensive
  and occasionally slow. `not_measured` states the omission explicitly instead of
  reporting a misleading `0`.
- **The log sinks reflect what the logger does, not configuration it ignores.**
  `LogInfo` is built from the same `LogConfig` that wires the subscriber, so the
  report and the wiring share one source of truth and cannot diverge. The binary
  writes the `PHOENIX_LOG_FILE` sink itself (a non-blocking append worker), so a
  reported file path is always one the process genuinely writes — never a mere
  launcher redirection the process cannot guarantee. A `PHOENIX_LOG_FILE` that
  cannot be opened aborts startup rather than degrading silently, so the report
  is only ever derived from `LogConfig` once every configured sink is actually
  installed — the report cannot advertise a sink the subscriber isn't writing.
  stdout and file are
  independent sinks; a deployment enables whichever it needs, or both. This is the
  single mechanism every launch path uses (dev, launchd, daemon, and systemd when
  configured), replacing the previous per-mode mix of shell/plist redirection and
  journald-only capture.
- **Read-only, single snapshot, no streaming.** The operator question is "what is
  it now," answered by a snapshot plus refresh. A live-streaming gauge would add
  an SSE surface for no proportional benefit.

## Cross-Spec Cross-References

- `specs/api/` — owns the HTTP router and auth middleware this endpoint registers
  under; `GET /api/deployment` follows the same JSON-handler + auth pattern.
- `specs/auth/` — the `/api/deployment` data endpoint is gated by the
  password-auth middleware like the other `/api/*` routes. The `/about` SPA
  route is auth-exempt (alongside `/new` and `/codex/login`) so a
  password-protected hard load can fetch the SPA shell before the client-side
  login check renders; the shell then calls the gated endpoint, which is denied
  until the user authenticates. The exemption covers only the static shell, not
  the data.
- `specs/conversation-ui/` — owns the settings dropdown and conversation-list
  chrome the "About this deployment" entry is added to.
