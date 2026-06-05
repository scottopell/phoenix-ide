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

LogInfo =                        // serde-tagged on `sink`
  | { sink: "file", path: string }   // a deployment-owned log file
  | { sink: "stdout" }               // logs go to stdout, captured by the supervisor
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
- `log: LogInfo` — the sink the logger actually writes to. The tracing layer
  writes only to stdout, so this is `LogInfo::Stdout`; `LogInfo::File` becomes
  reachable when the logger is wired to a process-owned file (REQ-DEPLOY-006).
- `locations: Vec<DiskLocation>` — the on-disk layout. Each `DiskLocation`
  carries a `label`, a resolved absolute `path`, and a `MeasureMode` dictating
  how it is sized at request time. The rows are: the database file (`File`); the
  data directory — recursed (`RecurseSmall`) only when it is Phoenix's own
  dedicated `.phoenix-ide` directory, otherwise reported path-only (`NoMeasure`)
  so a custom `PHOENIX_DB_PATH` cannot make a request walk `/tmp` or `$HOME`; the
  TLS inputs — the managed directory in auto mode (`RecurseSmall`) or the
  explicit certificate and key files in manual mode (`File` each); the built-in
  skills directory (`RecurseSmall`); the active codex credential file (`File`) —
  the one the process actually loads via `resolve_active_auth_path` (Phoenix's
  own `~/.phoenix-ide/codex-auth.json`, or Codex CLI's `~/.codex/auth.json` under
  `OPENAI_USE_CODEX_AUTH` piggyback mode); the attachment store (`InlineDb` while
  attachments live in the database); the browser binary cache (`NoMeasure`); and
  the per-scope browser profile glob (`Pattern`). Every path is resolved from the
  same logic the rest of the process uses so the page reports the locations the
  process actually uses.

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
3. **Disk:** one `DiskEntry` per configured `DiskLocation`, sized per its
   `MeasureMode` (see Config capture). `File`/`RecurseSmall` produce
   `DiskSize::measured`; `NoMeasure`/`Pattern` produce `DiskSize::not_measured`;
   a missing real path yields `DiskSize::absent`; the attachment store yields
   `inline_db`.
4. **`sampled_at`:** `Utc::now()` at the moment the snapshot is assembled.

The `dir_size` helper is bounded: it recurses only the directories the spec
classifies as small and never follows into the large-cache paths, so a single
request cannot trigger a multi-gigabyte walk (REQ-DEPLOY-005).

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
  "unavailable." TLS-disabled renders "Serving plain HTTP." `LogInfo.sink ==
  "stdout"` renders "stdout (captured by the supervising process)."
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
- **The log sink reflects what the logger does, not configuration it ignores.**
  The binary logs to stdout; any `.log` file is a launcher redirection the
  process does not own. Reporting a path the logger does not write would be a
  claim the process cannot keep, so `LogInfo` is derived from the logger's actual
  sink — currently always stdout. Honoring an intended log-file path is a
  separate capability the deployment can grow (it wires the logger and flips the
  reported sink in one change); until then the page reports stdout even if such a
  path is present in the environment.
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
