# Add exclusive OTLP tracing exporter for local Jaeger

## Goal

Add first-class OTLP trace export as an alternative to Phoenix's current Datadog trace exporter, so a local/prod Phoenix instance can send `tracing` spans to a local Jaeger instance without Datadog.

The exporters must be mutually exclusive: Phoenix exports spans to **Datadog OR OTLP OR nothing**, never both.

## Current state

Phoenix already bridges `tracing` spans into OpenTelemetry via `tracing-opentelemetry` in `crates/phoenix-ide/src/logging.rs`. The current provider construction is Datadog-specific:

- opt-in is controlled by `DD_TRACE_ENABLED`, `DD_TRACE_AGENT_URL`, or `DD_AGENT_HOST`;
- `datadog-opentelemetry` exports Datadog native v0.4 trace payloads over HTTP;
- `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_PROTOCOL` are currently ignored.

`TracingHandles` already stores an `SdkTracerProvider` and flushes it on shutdown, so the main shape can remain.

## Env contract

Introduce a Phoenix-owned exporter selector:

```bash
PHOENIX_TRACE_EXPORTER=none|datadog|otlp
```

Behavior:

- `PHOENIX_TRACE_EXPORTER=none`: force no trace export.
- `PHOENIX_TRACE_EXPORTER=datadog`: force current Datadog exporter path.
- `PHOENIX_TRACE_EXPORTER=otlp`: force OTLP exporter path.
- unset: preserve today's Datadog-compatible auto-opt-in behavior:
  - `DD_TRACE_ENABLED=false` / `0` disables;
  - otherwise Datadog export is requested by `DD_TRACE_ENABLED=true` / `1`, `DD_TRACE_AGENT_URL`, or `DD_AGENT_HOST`;
  - otherwise no trace export.

Do not make `OTEL_EXPORTER_OTLP_ENDPOINT` alone auto-enable tracing. Require `PHOENIX_TRACE_EXPORTER=otlp` so an unrelated environment does not unexpectedly turn on Phoenix export.

Example target config:

```bash
PHOENIX_TRACE_EXPORTER=otlp
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_SERVICE_NAME=phoenix-ide
```

If the final implementation supports gRPC instead/as well, document the matching config, usually:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
OTEL_EXPORTER_OTLP_PROTOCOL=grpc
```

## Dependency-bloat decision point

Before implementing the exporter, compare `opentelemetry-otlp` feature choices and choose the lowest-bloat option that works well with local Jaeger.

Constraints/preferences:

- Phoenix has mostly avoided gRPC/tonic dependencies. `Cargo.lock` currently contains `prost` transitively, but not `tonic`.
- Prefer OTLP HTTP/protobuf on Jaeger's `4318` endpoint if it avoids pulling in `tonic` and a large gRPC stack.
- Only choose OTLP gRPC on `4317` if the HTTP/protobuf path is materially worse, unsupported by compatible crate versions, or operationally unreliable.
- Capture the decision in local code comments only where they describe the chosen dependency feature; avoid broad design commentary in source. A short note in this task is sufficient for workflow context.

Likely dependency direction if HTTP/protobuf is viable:

```toml
opentelemetry-otlp = { version = "0.31", default-features = false, features = ["trace", "http-proto", "reqwest-client"] }
```

Adjust exact feature names to the published `opentelemetry-otlp` 0.31 API. If the SDK requires a runtime feature, add the minimal `opentelemetry_sdk` feature needed for batch export.

## Implementation plan

1. In `crates/phoenix-ide/src/logging.rs`, extract trace exporter selection into a small typed helper, e.g.:

   ```rust
   enum TraceExporter {
       None,
       Datadog,
       Otlp,
   }
   ```

   Keep env parsing testable without initializing global tracing.

2. Preserve the current Datadog provider path, including Phoenix fallbacks for:

   - `DD_SERVICE` -> `phoenix-ide`
   - `DD_ENV` -> `prod`
   - `DD_VERSION` -> `PHOENIX_VERSION` when available

3. Add an OTLP provider path that builds an `opentelemetry_sdk::trace::SdkTracerProvider` with an OTLP span exporter.

   Requirements:

   - respect `OTEL_EXPORTER_OTLP_ENDPOINT`;
   - respect the selected/supported protocol;
   - set `service.name` from `OTEL_SERVICE_NAME`, falling back to `DD_SERVICE`, then `phoenix-ide`;
   - set useful environment/version resource attrs from `OTEL_RESOURCE_ATTRIBUTES` and/or Phoenix fallbacks without creating a parallel conflicting representation.

4. Reuse the existing `tracing_opentelemetry::layer().with_tracer(provider.tracer("phoenix-ide"))` wiring and the existing `http.stream` span filter.

5. Reuse `TracingHandles::shutdown_tracer()` for OTLP provider shutdown. Update wording from Datadog-specific to generic trace exporter where appropriate.

6. Decide startup behavior:

   - malformed explicit `PHOENIX_TRACE_EXPORTER` value should fail startup with a clear error;
   - malformed explicit OTLP exporter config should fail startup;
   - collector unreachable after startup should not crash Phoenix; exporter retry/log behavior can be left to the OTel SDK.

## Validation

Manual validation target:

```bash
docker run --rm --name jaeger \
  -e COLLECTOR_OTLP_ENABLED=true \
  -p 16686:16686 \
  -p 4317:4317 \
  -p 4318:4318 \
  jaegertracing/all-in-one:latest
```

Then run Phoenix with the chosen OTLP config and hit an `/api/*` endpoint. Confirm spans appear at:

```text
http://localhost:16686
```

Automated checks:

- Unit-test exporter selection env parsing, including unset backwards-compatible Datadog auto-opt-in.
- Unit-test invalid `PHOENIX_TRACE_EXPORTER` handling.
- Run `./dev.py check`.

## Non-goals

- Exporting to Datadog and OTLP simultaneously.
- Metrics/logs OTLP export.
- Full inbound/outbound distributed trace propagation.
- Replacing Phoenix's structured JSON stdout/file logging.

## Acceptance criteria

- Existing Datadog tracing behavior is preserved when `PHOENIX_TRACE_EXPORTER` is unset.
- `PHOENIX_TRACE_EXPORTER=none` disables trace export even if Datadog env vars are present.
- `PHOENIX_TRACE_EXPORTER=otlp` sends Phoenix spans to a local Jaeger OTLP collector using the selected low-bloat protocol.
- The chosen OTLP dependency feature set avoids `tonic` unless the task explicitly determines gRPC is necessary.
- Shutdown flushes the active provider without Datadog-specific assumptions.
- `./dev.py check` passes.
