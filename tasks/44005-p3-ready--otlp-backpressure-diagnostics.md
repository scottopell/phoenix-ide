# Consider explicit OTLP backpressure diagnostics

Phoenix now supports OTLP trace export for local Jaeger. The OpenTelemetry SDK batch span processor can signal telemetry backpressure, but visibility depends on internal OTel logging features and log filters.

Point-in-time research:

- `opentelemetry_sdk 0.31` tracks dropped spans in the batch span processor when the queue is full or closed.
- On the first drop, it emits an internal warning named `BatchSpanProcessor.SpanDroppingStarted`.
- On clean shutdown, it emits `BatchSpanProcessor.Shutdown` with `dropped_spans` and `max_queue_size` if any spans were dropped.
- Export failures are logged through internal debug events such as `BatchSpanProcessor.Export.Error` and OTLP HTTP exporter events such as `HttpTracesClient.ExportFailed`.
- Default batch settings include `OTEL_BSP_MAX_QUEUE_SIZE=2048`, `OTEL_BSP_MAX_EXPORT_BATCH_SIZE=512`, `OTEL_BSP_SCHEDULE_DELAY=5000`, `OTEL_BSP_EXPORT_TIMEOUT=30000`, and `OTEL_BSP_MAX_CONCURRENT_EXPORTS=1`.

Possible future work:

- Explicitly enable OTel `internal-logs` features on `opentelemetry_sdk` and `opentelemetry-otlp` if we want drop/export diagnostics to reliably appear in Phoenix logs.
- Document a recommended diagnostic filter, e.g. `RUST_LOG=info,opentelemetry_sdk=debug,opentelemetry_otlp=debug`.
- Consider documenting local Jaeger timeout/backpressure env values such as `OTEL_BSP_MAX_QUEUE_SIZE=4096`, `OTEL_BSP_EXPORT_TIMEOUT=2000`, and `OTEL_EXPORTER_OTLP_TRACES_TIMEOUT=2000`.

No code change is needed unless OTLP trace loss becomes confusing or operationally relevant.
