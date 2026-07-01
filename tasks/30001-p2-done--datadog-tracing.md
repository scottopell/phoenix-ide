# Instrument Phoenix API with Datadog tracing (datadog-opentelemetry)

## Goal

Ship Phoenix's per-request HTTP spans to Datadog via the `datadog-opentelemetry`
crate (the real name behind the `ddtrace` docs.rs redirect). The existing
`tower-http` `TraceLayer` already emits an `http` span per request with
`method`/`path`/`status`/`latency_ms`; the `tracing-opentelemetry` bridge
reroutes those spans to the Datadog agent using the native v0.4 traces format
(msgpack over HTTP), not OTLP. No handler changes required.

## Background (from triage)

- **Crate:** `datadog-opentelemetry` v0.4.0, an OpenTelemetry SDK shim built on
  `opentelemetry` 0.31 + `tracing-opentelemetry` 0.32. MSRV 1.87 (Phoenix needs
  1.94 — fine).
- **Phoenix today:** `tracing` 0.1 + `tracing-subscriber` 0.3 across 10 crates.
  `crates/phoenix-ide/src/logging.rs::init()` builds the single global subscriber
  (`EnvFilter` + optional JSON stdout + optional file layer). `main.rs:911` has the
  `TraceLayer`. No opentelemetry deps exist yet — greenfield add.
- **Shutdown path:** `main.rs:1003` `tokio::select` on `shutdown_signal` → graceful
  drain → `shutdown_kill_tree`. Tracer provider shutdown hooks in here.
- **Config:** env-driven by default (`DD_SERVICE`, `DD_ENV`, `DD_VERSION`,
  `DD_TRACE_AGENT_URL`, `DD_TRACE_SAMPLE_RATE`, `DD_TRACE_ENABLED`).
  `Config::enabled()` gates whether spans export — defaults on.

## Plan — 3 touch points

### 1. `crates/phoenix-ide/Cargo.toml` — add deps

```toml
datadog-opentelemetry = "0.4"
opentelemetry = "0.31"
opentelemetry_sdk = "0.31"
tracing-opentelemetry = "0.32"
```

No feature flags needed for tracing-only (metrics/logs are opt-in features on the
crate; leave them off for v1).

### 2. `crates/phoenix-ide/src/logging.rs` — wire the OTel layer into `init()`

- Call `datadog_opentelemetry::tracing().init()` to build the `SdkTracerProvider`
  (reads `DD_*` env vars; sets the global tracer provider + text-map propagator).
- Build `tracing_opentelemetry::layer().with_tracer(provider.tracer("phoenix-ide"))`.
- Add that layer to the existing `tracing_subscriber::registry().with(...)` chain
  alongside `env_filter`, `stdout_layer`, `file_layer`.
- **Change `init()`'s return type** to also return the `SdkTracerProvider` so `main`
  can hold it for the process lifetime and shut it down on exit. Keep the existing
  `Option<WorkerGuard>` return; return both (e.g. a small struct or tuple).
- `init()` must remain the first thing `main` does (it sets globals). It already is.

### 3. `crates/phoenix-ide/src/main.rs` — hold + shutdown the provider

- Capture the `SdkTracerProvider` from the updated `logging::init()` alongside
  `_log_guard` (line ~558).
- On the shutdown branch (after the graceful drain, before/around
  `shutdown_kill_tree` at line ~1021), call
  `tracer_provider.shutdown_with_timeout(Duration::from_secs(1))` to flush
  in-flight spans to the agent. Log a warning on error but don't fail shutdown.

## Config / env

Default to env-driven config. For dev convenience, set a programmatic fallback
service name only if `DD_SERVICE` is unset — but prefer letting the env be the
source of truth (matches how `LogConfig` already works). Do **not** hardcode
`DD_ENV`/`DD_VERSION`.

## Out of scope (v1)

- **Outbound trace propagation** on `reqwest` calls to LLM providers (needs
  `opentelemetry-http` `HeaderInjector` on each outbound request). The inbound
  `TraceLayer` + bridge gives us server spans; distributed tracing across the
  LLM call boundary is a follow-up.
- **Inbound trace context extraction** — when Phoenix sits behind another
  instrumented service that sends `traceparent`/Datadog trace headers, a
  middleware is needed to extract the parent context from request headers so
  Datadog can stitch Phoenix into the upstream trace. Follow-up.
- **Metrics / logs** export (opt-in crate features; leave off).
- **Custom span attributes** beyond what `TraceLayer` already records.

## Verification

- `./dev.py check` passes (clippy + fmt + tests + codegen-stale guard).
- `./dev.py up` starts cleanly; `phoenix.log` shows no tracer-init errors.
- With a Datadog agent reachable (`DD_TRACE_AGENT_URL`), hitting any `/api/*`
  endpoint produces a server span in the Datadog UI with
  `service.name=phoenix-ide` (or whatever `DD_SERVICE` is set to), `http.method`,
  `http.route`, status, latency.
- With `DD_TRACE_ENABLED=false` (or no agent), the server runs normally and the
  OTel layer is a no-op — no errors, no span export attempts spamming logs.
- Shutdown flushes cleanly (no "tracer shutdown error" panic; spans appear in UI
  before the process exits).

## Gotchas

- `datadog_opentelemetry::tracing().init()` sets **global** tracer provider +
  propagator. Must run exactly once, before any tracing. Already first in `main`.
- On Linux, `init()` publishes tracer metadata to the OTel process context
  (eBPF profiler hook). No-op on macOS, harmless.
- The crate's `make_tracer` catches panics and falls back to a no-op provider —
  init failure degrades to "no traces" rather than crashing. Good.
- `tracing-opentelemetry` 0.32 requires `opentelemetry` 0.31 — pin both to match
  the published crate's deps to avoid version-skim compile errors.
# Validation Plan: Datadog Tracing Instrumentation

Validate that Phoenix's per-request HTTP spans are exported to the Datadog
trace agent endpoint with the expected shape. Uses the mock LLM model (no API
keys needed) and a local HTTP blackhole that captures trace payloads.

## Architecture

```
phoenix_ide  ──POST /v0.4/traces (msgpack)──▶  mock trace agent (blackhole)
  [DD_TRACE_AGENT_URL]                          captures + prints spans
```

The `datadog-opentelemetry` exporter uses libdatadog's native v0.4 traces
format (msgpack-encoded POST to the trace agent URL), **not** OTLP HTTP/gRPC.
Default endpoint is `http://localhost:8126`; override with `DD_TRACE_AGENT_URL`.

## Prerequisites

- Branch `task-30001-datadog-tracing` checked out
- `./dev.py check` passes (already verified)
- Python 3 with `msgpack` installed (`pip install msgpack` or `uv pip install msgpack`)
- No real Datadog agent running on port 8126 (or use a custom port)

## Step 1: Start the mock trace agent (blackhole)

A simple HTTP server that captures `/v0.4/traces` POSTs, decodes the msgpack
payload, and prints span data. Run this in a separate terminal:

```bash
uv run --with msgpack python3 - <<'PY'
from http.server import HTTPServer, BaseHTTPRequestHandler
import msgpack, json, sys, time

class TraceCapture(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(length)
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(b'{}')  # minimal agent response

        try:
            payload = msgpack.unpackb(body, raw=False, strict_map_key=False)
            # v0.4 format: list of traces, each trace is a list of spans
            traces = payload if isinstance(payload, list) else [payload]
            for trace in traces:
                spans = trace if isinstance(trace, list) else [trace]
                for span in spans:
                    print(json.dumps({
                        'name': span.get('name'),
                        'service': span.get('service'),
                        'resource': span.get('resource'),
                        'type': span.get('type'),
                        'duration_ns': span.get('duration'),
                        'error': span.get('error'),
                        'meta': span.get('meta', {}),
                        'metrics': span.get('metrics', {}),
                    }, indent=2))
                    print('---')
        except Exception as e:
            print(f'[decode error] {e}', file=sys.stderr)
            print(f'[raw {len(body)} bytes] {body[:200]}...', file=sys.stderr)

    def log_message(self, fmt, *args):
        ts = time.strftime('%H:%M:%S')
        print(f'[{ts}] {fmt % args}', file=sys.stderr)

print('Mock trace agent listening on http://localhost:9527', file=sys.stderr)
HTTPServer(('127.0.0.1', 9527), TraceCapture).serve_forever()
PY
```

This listens on port **9527** (avoids conflicting with a real agent on 8126).

## Step 2: Start Phoenix with tracing env vars

In a separate terminal, from the worktree root:

```bash
# Tracing config — point at our blackhole, set service name
export DD_TRACE_AGENT_URL=http://localhost:9527
export DD_SERVICE=phoenix-ide
export DD_ENV=dev
export DD_TRACE_ENABLED=true
export DD_TRACE_DEBUG=true   # verbose tracer logs in phoenix.log

# Start dev server (mock model is already enabled in .phoenix-ide.dev.env)
./dev.py up
```

Verify Phoenix started cleanly:

```bash
./dev.py status
# Should show: Phoenix: running (PID ...)
# URL: https://localhost:<port>  (dev uses self-signed TLS)
```

Check `phoenix.log` for tracer init errors:

```bash
grep -i 'tracer\|datadog\|ddtrace\|opentelemetry' phoenix.log | head -20
# Should NOT show: "Failed to initialize Datadog tracer provider"
# May show: agent connection attempts to localhost:9527
```

## Step 3: Drive a conversation via the CLI client

Use `phoenix-client.py` with the mock model to generate HTTP requests:

```bash
# Create a conversation and send a message (mock model, no API keys needed)
uv run phoenix-client.py --model mock "hello, trace me"

# Or if the dev server uses self-signed TLS:
uv run phoenix-client.py --model mock "hello, trace me" \
  --api-url https://localhost:<port>
```

This generates several HTTP requests to Phoenix's API:
- `POST /api/conversations/new` (create conversation)
- `POST /api/conversations/:id/chat` (send message)
- `GET /api/conversations/:id/stream` (SSE stream)
- `GET /api/conversations` (poll for state)

Each request should produce an `http` server span.

## Step 4: Validate span data in the blackhole

The mock trace agent terminal should print captured spans. Validate:

### 4a. Spans arrive at all

After the CLI client returns, the blackhole should have printed at least 2-3
span objects. If nothing arrives:
- Check `phoenix.log` for tracer errors
- Verify `DD_TRACE_AGENT_URL` is set in the Phoenix process env
- Check the blackhole is listening (`curl -X POST http://localhost:9527/v0.4/traces -d '{}'`)
- Wait a few seconds — the exporter batches and flushes periodically

### 4b. Expected span shape

Each span should have (verified against actual captured spans):

| Field | Expected value |
|---|---|
| `name` | `"internal"` (OTel span kind, mapped by the Datadog exporter) |
| `resource` | `"http"` (the `TraceLayer`'s `info_span!("http", ...)` name) |
| `service` | `"phoenix-ide"` (from `DD_SERVICE`) |
| `type` | `"custom"` (Datadog maps OTel `internal` kind to `custom`) |
| `duration_ns` | > 0 (request latency in nanoseconds) |
| `error` | `null` for 2xx responses |
| `meta.method` | `"GET"` or `"POST"` (no `http.` prefix) |
| `meta.path` | the request path, e.g. `/api/conversations/new` |
| `meta.env` | `"dev"` (from `DD_ENV`) |
| `meta.deployment.environment.name` | `"dev"` |
| `meta.span.kind` | `"internal"` |
| `meta.otel.scope.name` | `"phoenix-ide"` |
| `meta.telemetry.sdk.name` | `"datadog"` |
| `meta.telemetry.sdk.language` | `"rust"` |
| `meta.code.file.path` | `"crates/phoenix-ide/src/main.rs"` (TraceLayer location) |
| `metrics._sampling_priority_v1` | `1.0` (sampled) |
| `metrics.thread.id` | tokio worker thread ID |
| `metrics.busy_ns` / `metrics.idle_ns` | busy/idle time within the span |

Note: `meta.http.status_code` and `meta.http.latency_ms` are **not** present as
span attributes — the `TraceLayer`'s `on_response` callback logs them as events,
not span attributes. The method and path are in `meta.method` / `meta.path`
(without the `http.` prefix) because the `TraceLayer` sets them as span fields
directly, and the OTel bridge maps tracing fields to `meta.*`.

### 4c. Service name is correct

```bash
# In the blackhole output, verify:
grep '"service": "phoenix-ide"' # should match
```

### 4d. Span count matches request count

Roughly: each HTTP request to the Phoenix API = one span. The CLI client makes
~3-5 requests (create, chat, stream, poll). The span count should be in that
range. Health-check spans (`/version`) may be suppressed (the `TraceLayer`
uses `debug_span!` for those, which may be filtered by `EnvFilter`).

## Step 5: Validate disabled-tracing no-op

Verify that with tracing disabled, the server runs normally and the blackhole
receives nothing:

```bash
# Stop Phoenix
./dev.py down

# Disable tracing
export DD_TRACE_ENABLED=false

# Restart
./dev.py up

# Drive a conversation
uv run phoenix-client.py --model mock "should not be traced"

# Blackhole should receive NOTHING
# phoenix.log should not show span export attempts
```

## Step 6: Validate clean shutdown flush

Verify that in-flight spans are flushed on shutdown:

```bash
# With tracing re-enabled, start Phoenix, drive a conversation
export DD_TRACE_ENABLED=true
./dev.py up
uv run phoenix-client.py --model mock "flush on shutdown"

# Immediately stop Phoenix (triggers graceful shutdown + shutdown_tracer)
./dev.py down

# The blackhole should receive all spans BEFORE the process exits
# (shutdown_tracer calls shutdown_with_timeout(1s) which flushes the batch)
```

## Cleanup

```bash
./dev.py down
# Kill the mock trace agent (Ctrl-C in its terminal)
unset DD_TRACE_AGENT_URL DD_SERVICE DD_ENV DD_TRACE_ENABLED DD_TRACE_DEBUG
```

## Results (2026-07-01)

All 6 steps executed. Summary:

| Step | Result |
|---|---|
| 1. Mock trace agent | ✅ HTTP server on :9527, capturing `/v0.4/traces` POSTs |
| 2. Phoenix startup | ✅ No tracer init errors in `phoenix.log`; server started cleanly |
| 3. Drive conversation | ✅ `phoenix-client.py --model mock` completed; mock model responded |
| 4. Span capture | ✅ 3 spans captured: `GET /api/auth/status`, `POST /api/conversations/new`, `GET /api/conversations/:id/stream` |
| 5. Disabled no-op | ✅ `DD_TRACE_ENABLED=false` → zero trace spans in blackhole; conversation still worked |
| 6. Shutdown flush | ✅ Clean shutdown, no tracer errors; final telemetry heartbeat sent |

### Captured span example

```json
{
  "name": "internal",
  "service": "phoenix-ide",
  "resource": "http",
  "type": "custom",
  "duration_ns": 547229000,
  "error": null,
  "meta": {
    "method": "POST",
    "path": "/api/conversations/new",
    "env": "dev",
    "span.kind": "internal",
    "otel.scope.name": "phoenix-ide",
    "telemetry.sdk.name": "datadog",
    "telemetry.sdk.language": "rust",
    "code.file.path": "crates/phoenix-ide/src/main.rs"
  },
  "metrics": {
    "_sampling_priority_v1": 1.0,
    "thread.id": 10.0,
    "busy_ns": 546713289.0,
    "idle_ns": 514586.0
  }
}
```

### Additional observations

- The tracer also sends **telemetry** heartbeats (`/telemetry/proxy/api/v2/apmtelemetry`)
  every ~60s and **remote config** polls (`/v0.7/config`) every ~5s to the same
  agent URL. These are JSON, not msgpack, so the blackhole's msgpack decoder logs
  harmless decode errors for them.
- The exporter batches spans and flushes periodically (~every few seconds during
  active requests). Spans arrive within a few seconds of the HTTP request completing.
- With `DD_TRACE_ENABLED=false`, the tracer provider is still initialized (globals
  are set) but no `DatadogSpanProcessor` is installed, so spans go nowhere — a
  clean no-op with zero network traffic.

