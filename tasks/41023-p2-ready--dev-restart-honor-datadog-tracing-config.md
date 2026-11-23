# Development restart omits configured Datadog tracing support

Observed 2026-11-23 while restarting an idle checkout after Codex tool-search qualification. Local configuration selects PHOENIX_TRACE_EXPORTER=datadog. ./dev.py restart builds plain cargo build; the resulting binary omits phoenix_ide/datadog-tracing. The launcher reports startup after a 0.5-second process check, but the server subsequently exits with: Datadog trace export requested, but this Phoenix build omits datadog-tracing support.

Investigate feature selection for local builds and startup readiness. Preserve configured tracing rather than silently disabling it. Reproduce with a Datadog-configured dev environment, confirm restart produces a compatible binary and serves its API, and cover the launcher falsely reporting a start when initialization exits shortly afterward.

Immediate restoration uses dev.py's build/start helpers with an explicit phoenix_ide/datadog-tracing build feature, preserving the same database, ports, TLS and tracing configuration. Production deployment was not touched.
