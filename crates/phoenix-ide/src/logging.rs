//! Logging configuration and initialization.
//!
//! Phoenix supports two independent log sinks, each toggled by its own env var:
//!
//! - `PHOENIX_LOG_STDOUT` (bool, default `true`) — structured JSON to stdout.
//! - `PHOENIX_LOG_FILE` (optional path) — structured JSON appended to that file,
//!   written by the process itself via a non-blocking worker thread.
//!
//! Both may be enabled at once (logs fan out to every enabled sink), so a
//! deployment chooses whatever is appropriate. [`LogConfig`] is resolved once
//! from the environment and is the single source of truth for both what the
//! subscriber writes and what `GET /api/deployment` reports (via
//! [`LogConfig::to_log_info`]) — the report cannot drift from the wiring.

use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanLimits};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// The resolved set of enabled log sinks.
pub struct LogConfig {
    /// Whether structured logs are written to stdout.
    pub stdout: bool,
    /// Path of the process-owned log file, when file logging is enabled.
    pub file: Option<PathBuf>,
}

/// Handles that must outlive the process so background workers flush on shutdown.
///
/// - `_log_guard` — the non-blocking file appender worker (when file logging is on).
/// - `tracer_provider` — the active `OTel` tracer provider, held so spans flush
///   before exit. Call [`TracingHandles::shutdown_tracer`] during graceful
///   shutdown.
pub struct TracingHandles {
    _log_guard: Option<WorkerGuard>,
    tracer_provider: Option<SdkTracerProvider>,
}

impl TracingHandles {
    /// Flush in-flight spans to the active trace exporter. Bounded by a 1s
    /// timeout so a stuck collector cannot delay shutdown indefinitely. Logs a
    /// warning on error but never fails shutdown. No-op when tracing was not
    /// opted in.
    pub fn shutdown_tracer(&self) {
        if let Some(provider) = &self.tracer_provider {
            if let Err(e) = provider.shutdown_with_timeout(Duration::from_secs(1)) {
                tracing::warn!(error = ?e, "tracer shutdown error");
            }
        }
    }
}

impl LogConfig {
    /// Resolve the sink configuration from the environment.
    pub fn from_env() -> Self {
        Self {
            stdout: stdout_enabled(std::env::var("PHOENIX_LOG_STDOUT").ok().as_deref()),
            file: std::env::var_os("PHOENIX_LOG_FILE")
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty()),
        }
    }

    /// The deployment-report view of the active sinks, with the file path made
    /// absolute. Derived from the same values used to build the subscriber.
    pub fn to_log_info(&self) -> crate::api::LogInfo {
        crate::api::LogInfo {
            stdout: self.stdout,
            file: self
                .file
                .as_deref()
                .map(|p| crate::api::absolutize(p).display().to_string()),
        }
    }
}

/// Build and install the global tracing subscriber for the configured sinks.
///
/// Returns [`TracingHandles`] which the caller MUST hold for the process
/// lifetime so the file appender worker and active tracer provider flush on
/// shutdown. Call [`TracingHandles::shutdown_tracer`] during graceful
/// shutdown to flush in-flight spans.
///
/// Fails (aborting startup) when `PHOENIX_LOG_FILE` is set but cannot be
/// opened. Honoring the configured sinks exactly — or refusing to start — is
/// what lets the deployment report derive its file path from [`LogConfig`]
/// without ever advertising a sink the subscriber isn't writing (REQ-DEPLOY-006).
pub fn init(config: &LogConfig) -> std::io::Result<TracingHandles> {
    let tracer_provider = match trace_exporter_from_env()? {
        TraceExporter::None => None,
        TraceExporter::Datadog => Some(init_datadog_provider()),
        TraceExporter::Otlp => Some(init_otlp_provider()?),
    };

    // OTel has its own allowlist instead of inheriting RUST_LOG. Local sinks may
    // opt into verbose dependency diagnostics, but exported traces contain only
    // Phoenix's intentional, bounded spans and never tracing events.
    let otel_layer = tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("phoenix-ide"))
            .with_filter(tracing_subscriber::filter::filter_fn(otel_metadata_enabled))
    });

    // EnvFilter is applied per-layer to the stdout/file sinks only, not to the
    // OTel layer. EnvFilter does not implement Clone, so we create a fresh
    // instance for each sink via make_env_filter().
    let stdout_layer = config.stdout.then(|| {
        fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .with_filter(make_env_filter())
    });

    let (file_layer, guard) = match config.file.as_deref() {
        Some(path) => {
            let file = open_append(path).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("failed to open PHOENIX_LOG_FILE {}: {e}", path.display()),
                )
            })?;
            let (writer, guard) = tracing_appender::non_blocking(file);
            let layer = fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .with_writer(writer)
                .with_filter(make_env_filter());
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(otel_layer)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    install_panic_hook();
    Ok(TracingHandles {
        _log_guard: guard,
        tracer_provider,
    })
}

const OTEL_SPAN_NAMES: &[&str] = &["http", "conversation.turn", "llm.request", "tool.execute"];

fn otel_metadata_enabled(meta: &tracing::Metadata<'_>) -> bool {
    meta.is_span() && OTEL_SPAN_NAMES.contains(&meta.name())
}

fn phoenix_span_limits() -> SpanLimits {
    SpanLimits {
        max_events_per_span: 0,
        max_attributes_per_span: 32,
        max_links_per_span: 4,
        max_attributes_per_event: 0,
        max_attributes_per_link: 4,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TraceExporter {
    None,
    Datadog,
    Otlp,
}

fn trace_exporter_from_env() -> std::io::Result<TraceExporter> {
    match std::env::var("PHOENIX_TRACE_EXPORTER") {
        Ok(value) => parse_explicit_trace_exporter(&value),
        Err(std::env::VarError::NotPresent) => Ok(datadog_auto_exporter_from_env()),
        Err(std::env::VarError::NotUnicode(_)) => invalid_input(
            "PHOENIX_TRACE_EXPORTER must be valid UTF-8 (expected none, datadog, or otlp)",
        ),
    }
}

fn parse_explicit_trace_exporter(value: &str) -> std::io::Result<TraceExporter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(TraceExporter::None),
        "datadog" => Ok(TraceExporter::Datadog),
        "otlp" => Ok(TraceExporter::Otlp),
        other => invalid_input(format!(
            "invalid PHOENIX_TRACE_EXPORTER={other:?}; expected none, datadog, or otlp"
        )),
    }
}

fn datadog_auto_exporter_from_env() -> TraceExporter {
    if env_var_is_false("DD_TRACE_ENABLED") {
        return TraceExporter::None;
    }

    if env_var_is_true("DD_TRACE_ENABLED")
        || std::env::var_os("DD_TRACE_AGENT_URL").is_some()
        || std::env::var_os("DD_AGENT_HOST").is_some()
    {
        TraceExporter::Datadog
    } else {
        TraceExporter::None
    }
}

fn init_datadog_provider() -> SdkTracerProvider {
    let mut dd_config_builder = datadog_opentelemetry::configuration::Config::builder();
    if std::env::var("DD_SERVICE").is_err() {
        dd_config_builder.set_service("phoenix-ide".to_string());
    }
    if std::env::var("DD_ENV").is_err() {
        dd_config_builder.set_env("prod".to_string());
    }
    if std::env::var("DD_VERSION").is_err() {
        if let Ok(version) = std::env::var("PHOENIX_VERSION") {
            dd_config_builder.set_version(version);
        }
    }

    datadog_opentelemetry::tracing()
        .with_config(dd_config_builder.build())
        .with_span_limits(phoenix_span_limits())
        .init()
}

fn init_otlp_provider() -> std::io::Result<SdkTracerProvider> {
    let protocol = otlp_protocol_from_env()?;
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(otlp_resource())
        .with_span_limits(phoenix_span_limits())
        .build())
}

fn otlp_protocol_from_env() -> std::io::Result<Protocol> {
    for name in [
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
    ] {
        match std::env::var(name) {
            Ok(value) => return parse_otlp_protocol(name, &value),
            Err(std::env::VarError::NotPresent) => {}
            Err(std::env::VarError::NotUnicode(_)) => {
                return invalid_input(format!(
                    "{name} must be valid UTF-8 (expected http/protobuf)"
                ));
            }
        }
    }
    Ok(Protocol::HttpBinary)
}

fn parse_otlp_protocol(name: &str, value: &str) -> std::io::Result<Protocol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "http/protobuf" => Ok(Protocol::HttpBinary),
        "grpc" => invalid_input(format!(
            "OTLP gRPC is not compiled into Phoenix; use {name}=http/protobuf and port 4318"
        )),
        other => invalid_input(format!(
            "unsupported {name}={other:?}; expected http/protobuf"
        )),
    }
}

fn otlp_resource() -> Resource {
    let mut attributes = Vec::new();
    if !otel_resource_attribute_is_set("deployment.environment") {
        if let Some(env) = non_empty_env("DD_ENV") {
            attributes.push(KeyValue::new("deployment.environment", env));
        }
    }
    if !otel_resource_attribute_is_set("service.version") {
        if let Some(version) =
            non_empty_env("DD_VERSION").or_else(|| non_empty_env("PHOENIX_VERSION"))
        {
            attributes.push(KeyValue::new("service.version", version));
        }
    }

    Resource::builder()
        .with_service_name(otlp_service_name())
        .with_attributes(attributes)
        .build()
}

fn otlp_service_name() -> String {
    non_empty_env("OTEL_SERVICE_NAME")
        .or_else(|| otel_resource_attribute_value("service.name"))
        .or_else(|| non_empty_env("DD_SERVICE"))
        .unwrap_or_else(|| "phoenix-ide".to_string())
}

fn otel_resource_attribute_is_set(key: &str) -> bool {
    otel_resource_attribute_value(key).is_some()
}

fn otel_resource_attribute_value(key: &str) -> Option<String> {
    std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .ok()?
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .find_map(|(name, value)| (name.trim() == key).then(|| value.trim().to_string()))
        .filter(|value| !value.is_empty())
}

fn env_var_is_true(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false)
}

fn env_var_is_false(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0"))
        .unwrap_or(false)
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn invalid_input<T>(message: impl Into<String>) -> std::io::Result<T> {
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

/// Create a fresh `EnvFilter` from the environment. Used per-sink because
/// `EnvFilter` does not implement Clone.
fn make_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "phoenix_ide=debug,tower_http=debug".into())
}

/// Whether the stdout sink is enabled. Defaults on (unset); only explicit
/// falsey values disable it.
fn stdout_enabled(var: Option<&str>) -> bool {
    var.is_none_or(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no" | ""
        )
    })
}

/// Open `path` for appending, creating it and any missing parent directories.
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// Route panics through `tracing` so they reach every configured sink (notably
/// the log file), then chain to the previous hook so the default stderr message
/// is preserved.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
        let message = info.payload().downcast_ref::<&str>().map_or_else(
            || {
                info.payload()
                    .downcast_ref::<String>()
                    .map_or_else(|| "Box<dyn Any>".to_string(), Clone::clone)
            },
            |s| (*s).to_string(),
        );
        tracing::error!(panic.location = %location, panic.message = %message, "panic");
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn otel_filter_is_a_spans_only_allowlist() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::InMemorySpanExporter;
        use tracing_subscriber::prelude::*;

        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .with_span_limits(phoenix_span_limits())
            .build();
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("test"))
            .with_filter(tracing_subscriber::filter::filter_fn(otel_metadata_enabled));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let llm = tracing::info_span!("llm.request", model = "gpt-test");
            let _guard = llm.enter();
            tracing::debug!(target: "tokio_tungstenite", frame = "PAYLOAD_SENTINEL", "frame");
            tracing::info!(delta = "DELTA_SENTINEL", "response delta");
            let dependency =
                tracing::debug_span!(target: "sqlx::query", "query", sql = "SELECT secret");
            drop(dependency);
        });
        provider.force_flush().expect("flush spans");

        let spans = exporter.get_finished_spans().expect("exported spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "llm.request");
        assert!(spans[0].events.is_empty());
        let encoded = format!("{spans:?}");
        for forbidden in [
            "PAYLOAD_SENTINEL",
            "DELTA_SENTINEL",
            "SELECT secret",
            "authorization",
        ] {
            assert!(!encoded.contains(forbidden), "export contained {forbidden}");
        }
    }

    #[test]
    fn otel_limits_are_conservative_and_event_free() {
        let limits = phoenix_span_limits();
        assert_eq!(limits.max_events_per_span, 0);
        assert_eq!(limits.max_attributes_per_event, 0);
        assert!(limits.max_attributes_per_span <= 32);
        assert!(limits.max_links_per_span <= 4);
    }

    #[test]
    fn stdout_defaults_on_when_unset() {
        assert!(stdout_enabled(None));
    }

    #[test]
    fn stdout_disabled_by_falsey_values() {
        for v in ["0", "false", "off", "no", "", "FALSE", " Off "] {
            assert!(!stdout_enabled(Some(v)), "{v:?} should disable stdout");
        }
    }

    #[test]
    fn stdout_enabled_by_truthy_values() {
        for v in ["1", "true", "on", "yes", "anything"] {
            assert!(stdout_enabled(Some(v)), "{v:?} should enable stdout");
        }
    }

    #[test]
    fn to_log_info_absolutizes_the_file_and_carries_stdout() {
        let cfg = LogConfig {
            stdout: false,
            file: Some(PathBuf::from("relative.log")),
        };
        let info = cfg.to_log_info();
        assert!(!info.stdout);
        let file = info.file.expect("file sink");
        assert!(std::path::Path::new(&file).is_absolute());
        assert!(file.ends_with("relative.log"));
    }

    #[test]
    fn open_append_errors_when_parent_is_not_a_directory() {
        // A requested file under a path whose parent is a regular file cannot be
        // created; init turns this Err into a startup abort (fail fast).
        let blocker =
            std::env::temp_dir().join(format!("phoenix-logging-test-{}", std::process::id()));
        std::fs::write(&blocker, b"x").unwrap();
        let unopenable = blocker.join("nested.log");
        assert!(open_append(&unopenable).is_err());
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn to_log_info_reports_no_file_when_unset() {
        let cfg = LogConfig {
            stdout: true,
            file: None,
        };
        let info = cfg.to_log_info();
        assert!(info.stdout);
        assert!(info.file.is_none());
    }

    #[test]
    fn explicit_trace_exporter_values_parse() {
        assert_eq!(
            parse_explicit_trace_exporter("none").unwrap(),
            TraceExporter::None
        );
        assert_eq!(
            parse_explicit_trace_exporter(" DATADOG ").unwrap(),
            TraceExporter::Datadog
        );
        assert_eq!(
            parse_explicit_trace_exporter("otlp").unwrap(),
            TraceExporter::Otlp
        );
    }

    #[test]
    fn invalid_explicit_trace_exporter_is_rejected() {
        let err = parse_explicit_trace_exporter("jaeger").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("PHOENIX_TRACE_EXPORTER"));
    }

    #[test]
    fn unset_trace_exporter_preserves_datadog_auto_opt_in() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        without_trace_export_env(|| {
            assert_eq!(trace_exporter_from_env().unwrap(), TraceExporter::None);

            std::env::set_var("DD_TRACE_ENABLED", "true");
            assert_eq!(trace_exporter_from_env().unwrap(), TraceExporter::Datadog);
            std::env::remove_var("DD_TRACE_ENABLED");

            std::env::set_var("DD_TRACE_AGENT_URL", "http://localhost:8126");
            assert_eq!(trace_exporter_from_env().unwrap(), TraceExporter::Datadog);
            std::env::remove_var("DD_TRACE_AGENT_URL");

            std::env::set_var("DD_AGENT_HOST", "localhost");
            assert_eq!(trace_exporter_from_env().unwrap(), TraceExporter::Datadog);
        });
    }

    #[test]
    fn explicit_none_disables_datadog_auto_opt_in() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        without_trace_export_env(|| {
            std::env::set_var("PHOENIX_TRACE_EXPORTER", "none");
            std::env::set_var("DD_TRACE_ENABLED", "true");
            std::env::set_var("DD_TRACE_AGENT_URL", "http://localhost:8126");
            assert_eq!(trace_exporter_from_env().unwrap(), TraceExporter::None);
        });
    }

    fn without_trace_export_env(test: impl FnOnce()) {
        for name in [
            "PHOENIX_TRACE_EXPORTER",
            "DD_TRACE_ENABLED",
            "DD_TRACE_AGENT_URL",
            "DD_AGENT_HOST",
        ] {
            std::env::remove_var(name);
        }
        test();
        for name in [
            "PHOENIX_TRACE_EXPORTER",
            "DD_TRACE_ENABLED",
            "DD_TRACE_AGENT_URL",
            "DD_AGENT_HOST",
        ] {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn otlp_http_protobuf_protocol_is_supported() {
        assert_eq!(
            parse_otlp_protocol("OTEL_EXPORTER_OTLP_PROTOCOL", "").unwrap(),
            Protocol::HttpBinary
        );
        assert_eq!(
            parse_otlp_protocol("OTEL_EXPORTER_OTLP_PROTOCOL", " http/protobuf ").unwrap(),
            Protocol::HttpBinary
        );
    }

    #[test]
    fn otlp_grpc_protocol_is_rejected_without_tonic() {
        let err = parse_otlp_protocol("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("gRPC is not compiled"));
    }
}
