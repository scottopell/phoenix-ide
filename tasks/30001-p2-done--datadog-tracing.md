# Instrument Phoenix API with Datadog tracing (datadog-opentelemetry)

## Goal

Ship Phoenix's per-request HTTP spans to Datadog via the `datadog-opentelemetry`
crate (the real name behind the `ddtrace` docs.rs redirect; local checkout at
`/Users/scott.opell/dd/dd-trace-rs`). The existing `tower-http` `TraceLayer`
already emits an `http` span per request with `method`/`path`/`status`/`latency_ms`;
the `tracing-opentelemetry` bridge reroutes those spans to the Datadog agent as
OTLP. No handler changes required.

## Background (from triage)

- **Crate:** `datadog-opentelemetry` v0.4.0, an OpenTelemetry SDK shim built on
  `opentelemetry` 0.32 + `tracing-opentelemetry` 0.33. MSRV 1.87 (Phoenix needs
  1.94 — fine).
- **Phoenix today:** `tracing` 0.1 + `tracing-subscriber` 0.3 across 10 crates.
  `crates/phoenix-ide/src/logging.rs::init()` builds the single global subscriber
  (`EnvFilter` + optional JSON stdout + optional file layer). `main.rs:911` has the
  `TraceLayer`. No opentelemetry deps exist yet — greenfield add.
- **Shutdown path:** `main.rs:1003` `tokio::select` on `shutdown_signal` → graceful
  drain → `shutdown_kill_tree`. Tracer provider shutdown hooks in here.
- **Config:** env-driven by default (`DD_SERVICE`, `DD_ENV`, `DD_VERSION`,
  `DD_AGENT_URL`, `DD_TRACE_SAMPLE_RATE`, `DD_TRACE_ENABLED`). `Config::enabled()`
  gates whether spans export — defaults on.

## Plan — 3 touch points

### 1. `crates/phoenix-ide/Cargo.toml` — add deps

```toml
datadog-opentelemetry = "0.4"
opentelemetry = "0.32"
tracing-opentelemetry = "0.33"
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
- **Metrics / logs** export (opt-in crate features; leave off).
- **Custom span attributes** beyond what `TraceLayer` already records.

## Verification

- `./dev.py check` passes (clippy + fmt + tests + codegen-stale guard).
- `./dev.py up` starts cleanly; `phoenix.log` shows no tracer-init errors.
- With a Datadog agent reachable (`DD_AGENT_URL`), hitting any `/api/*` endpoint
  produces a server span in the Datadog UI with `service.name=phoenix-ide` (or
  whatever `DD_SERVICE` is set to), `http.method`, `http.route`, status, latency.
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
- `tracing-opentelemetry` 0.33 requires `opentelemetry` 0.32 — pin both to match
  the crate's workspace deps to avoid version-skim compile errors.
