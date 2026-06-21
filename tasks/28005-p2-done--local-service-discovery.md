# Local Service Discovery via API Catalog

## Summary

Add a lightweight local-service discovery feature to Phoenix IDE. The backend should quietly probe selected loopback ports for `/.well-known/api-catalog`, parse valid RFC 9727 `application/linkset+json` responses, maintain an active registry of discovered services, and expose them to the UI through a small “Local Services” panel in the left sidebar.

This feature lets local mock servers, debug routers, and personal localhost apps appear automatically in Phoenix when they opt in by exposing a standard API catalog endpoint.

## Goals

- Discover explicit local services that self-advertise via RFC 9727 API catalogs.
- Keep discovery silent, bounded, and non-blocking.
- Track service lifecycle: found, healthy, stale, gone.
- Surface discovered services in a compact sidebar panel.
- Avoid broad or aggressive port scanning.
- Avoid guessing too much in the first version.

## Non-goals for the first implementation

- No OpenAPI import/tool generation yet.
- No launchd integration yet.
- No `$HOME/.local` hint-file system yet.
- No full 1–65535 port sweep by default.
- No LAN/private-network discovery.
- No root sniffing of arbitrary HTTP services yet.
- No automatic trust or execution of discovered links.

## Proposed backend architecture

Add a discovery subsystem under the Phoenix backend, likely shaped like:

```text
crates/phoenix-ide/src/discovery.rs
crates/phoenix-ide/src/discovery/
  config.rs
  supervisor.rs
  sweep.rs
  probe.rs
  linkset.rs
  registry.rs
  api.rs
```

The subsystem should have four main parts:

1. Discovery supervisor
   - Owns lifecycle of discovery workers.
   - Starts on backend startup when discovery is enabled.
   - Shuts down cleanly with the server.

2. Port sweeper
   - Periodically creates probe targets from conservative loopback port ranges.
   - Prioritizes recently discovered ports.
   - Adds jitter/backoff so probes do not all fire at once.

3. Probe executor
   - Sends fast HTTP requests to:

     ```text
     http://127.0.0.1:{port}/.well-known/api-catalog
     http://[::1]:{port}/.well-known/api-catalog
     ```

   - Uses strict timeouts.
   - Uses bounded concurrency.
   - Uses a bounded queue and drops probe work under load rather than blocking Phoenix.
   - Does not follow redirects by default.
   - Limits response body size.

4. Discovery registry
   - Maintains the current discovered-service snapshot.
   - Deduplicates observations.
   - Tracks `first_seen_at`, `last_seen_at`, status, and capabilities.
   - Exposes the current snapshot to API/UI.

## Suggested defaults

Initial defaults should be conservative:

```text
enabled: true in dev, configurable for prod
hosts: 127.0.0.1, ::1
probe timeout: 100–300ms
max concurrent probes: 16–64
sweep interval: 30–120s
body limit: small, e.g. 64 KiB
```

Default port ranges should be curated instead of exhaustive, for example:

```text
3000-3010
5000-5010
5173-5180
8000-8099
9000-9099
```

Recently discovered ports should be rechecked more frequently than broad ranges.

## Probe behavior

For each candidate port, send:

```http
GET /.well-known/api-catalog
Accept: application/linkset+json, application/json;q=0.8
User-Agent: phoenix-ide-local-discovery
```

A service is discovered only when:

- The response status is successful.
- The response content type is `application/linkset+json` or compatible JSON.
- The response body is within the configured size limit.
- The body parses as a valid linkset.

The first version should only record explicit API catalog services. Root sniffing can be added later as a lower-confidence discovery mode.

## Service model sketch

Represent discovered services structurally instead of passing around raw JSON as the primary state.

```rust
pub struct DiscoveredService {
    pub id: DiscoveredServiceId,
    pub base_url: Url,
    pub host: IpAddr,
    pub port: u16,
    pub title: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<ServiceCapability>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub status: ServiceStatus,
    pub confidence: DiscoveryConfidence,
    pub source: DiscoverySource,
}
```

Potential enums:

```rust
pub enum ServiceStatus {
    Healthy,
    Stale,
    Gone,
}

pub enum DiscoveryConfidence {
    ExplicitApiCatalog,
}

pub enum DiscoverySource {
    LoopbackProbe,
}

pub enum ServiceCapability {
    ApiCatalog { url: Url },
    OpenApi { url: Url, title: Option<String>, content_type: Option<String> },
    Documentation { url: Url, title: Option<String> },
    HtmlUi { url: Url, title: Option<String> },
    OtherLink { rel: String, url: Url, title: Option<String>, content_type: Option<String> },
}
```

The registry lifecycle can be:

```text
observed valid catalog -> Healthy
missed for stale_after -> Stale
missed for gone_after  -> Gone/remove from active list
observed again         -> Healthy
```

A single failed probe should not immediately remove a service.

## API surface

Add a read endpoint for the UI:

```text
GET /api/discovery/services
```

It returns the active discovered-service snapshot.

Optional follow-ups, not required for MVP:

```text
POST /api/discovery/services/:id/refresh
POST /api/discovery/services/:id/hide
POST /api/discovery/services/:id/pin
```

If Phoenix already has a suitable SSE/state-refresh pattern, add a discovery update event so the panel can update without polling. Otherwise, polling the endpoint is acceptable for the first cut.

## UI plan

Add a compact “Local Services” section to the left sidebar near project/context controls, not in the chat stream.

Preferred placement:

```text
[Project / Worktree]
[Model / Mode]
[Local Services]
[Conversations]
```

Default collapsed state:

```text
LOCAL SERVICES  2
```

Expanded state:

```text
LOCAL SERVICES  2

✓ debug-router   :8787
  OpenAPI · Docs

✓ notes-app      :4319
  UI
```

For MVP actions:

- Copy URL
- Open service/docs link where safe

Later actions:

- Attach service to conversation context
- Hide
- Pin
- Trust
- Import OpenAPI

The panel should be quiet and compact. It should not become a dashboard or modal.

## Locality and safety requirements

Discovery is from the Phoenix backend host, not necessarily the browser machine. UI wording should avoid implying that a discovered localhost service exists on the viewer’s machine when Phoenix is accessed remotely.

Rules:

- Probe loopback only by default.
- Do not probe LAN/private networks by default.
- Do not follow arbitrary links from catalogs during discovery.
- Do not execute anything based on discovery.
- Do not treat catalog links as trusted.
- Do not expose host-local OS actions to remote browsers.
- Keep logs at debug level for routine probe failures.

## Implementation phases

### Phase 1: backend explicit discovery

- Add discovery config.
- Add supervisor/background worker.
- Add bounded loopback sweep.
- Probe `/.well-known/api-catalog` only.
- Parse RFC 9727 linkset JSON enough to extract service links.
- Maintain in-memory registry.
- Expose `GET /api/discovery/services`.

### Phase 2: sidebar UI

- Add “Local Services” section to left sidebar.
- Fetch discovered services.
- Render compact collapsed/expanded list.
- Show service name/title, port, and basic capabilities.
- Add copy/open affordances where safe.

### Phase 3: lifecycle polish

- Add stale/gone transitions.
- Add SSE or efficient refresh.
- Recheck recently discovered ports more frequently.
- Add hide/pin preferences if noise appears.

### Phase 4: future extensions

- Root sniffing as lower-confidence discovery.
- OpenAPI import actions.
- Local hint files for launchd or `$HOME/.local` apps.
- User-configurable port ranges.

## Acceptance criteria

- A local test server exposing `/.well-known/api-catalog` appears in Phoenix without manual configuration.
- Discovery runs in the background without blocking normal IDE use.
- Probe concurrency and timeouts are bounded.
- Non-responsive ports do not produce noisy logs or UI errors.
- Services disappear or become stale after they stop responding.
- The UI shows discovered services in a small left-sidebar panel.
- The first version only shows explicit API-catalog services, avoiding noisy inferred results.
