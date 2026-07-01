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
use opentelemetry_sdk::trace::SdkTracerProvider;
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
/// - `tracer_provider` — the Datadog `OTel` tracer provider, held so spans flush
///   to the agent before exit. Call [`TracingHandles::shutdown_tracer`] during
///   graceful shutdown.
pub struct TracingHandles {
    _log_guard: Option<WorkerGuard>,
    tracer_provider: Option<SdkTracerProvider>,
}

impl TracingHandles {
    /// Flush in-flight spans to the Datadog agent. Bounded by a 1s timeout so a
    /// stuck agent cannot delay shutdown indefinitely. Logs a warning on error
    /// but never fails shutdown. No-op when tracing was not opted in.
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
/// lifetime so the file appender worker and the Datadog tracer provider flush
/// on shutdown. Call [`TracingHandles::shutdown_tracer`] during graceful
/// shutdown to flush in-flight spans.
///
/// Fails (aborting startup) when `PHOENIX_LOG_FILE` is set but cannot be
/// opened. Honoring the configured sinks exactly — or refusing to start — is
/// what lets the deployment report derive its file path from [`LogConfig`]
/// without ever advertising a sink the subscriber isn't writing (REQ-DEPLOY-006).
pub fn init(config: &LogConfig) -> std::io::Result<TracingHandles> {
    // Only initialize the Datadog OTel layer when tracing is explicitly
    // opted in. This prevents dev/self-hosted installs without a Datadog
    // agent from creating an exporter that spams localhost:8126 with trace
    // and telemetry traffic. Opt-in requires any of:
    //   - DD_TRACE_ENABLED=true (explicit)
    //   - DD_TRACE_AGENT_URL is set
    //   - DD_AGENT_HOST is set
    //   - DD_TRACE_AGENT_HOST is set
    let dd_tracing_requested = std::env::var("DD_TRACE_ENABLED")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
        || std::env::var("DD_TRACE_AGENT_URL").is_ok()
        || std::env::var("DD_AGENT_HOST").is_ok()
        || std::env::var("DD_TRACE_AGENT_HOST").is_ok();

    let tracer_provider = if dd_tracing_requested {
        // Fall back to "phoenix-ide" as the service name when DD_SERVICE is
        // not set, so traces land under the expected service without
        // requiring every deployment to set the env var.
        let mut dd_config_builder = datadog_opentelemetry::configuration::Config::builder();
        if std::env::var("DD_SERVICE").is_err() {
            dd_config_builder.set_service("phoenix-ide".to_string());
        }
        Some(
            datadog_opentelemetry::tracing()
                .with_config(dd_config_builder.build())
                .init(),
        )
    } else {
        None
    };

    // The OTel layer is intentionally unfiltered: it must always process
    // spans for export regardless of RUST_LOG/EnvFilter, so that quiet logging
    // configurations (e.g. RUST_LOG=warn) do not silently disable tracing.
    let otel_layer = tracer_provider
        .as_ref()
        .map(|provider| tracing_opentelemetry::layer().with_tracer(provider.tracer("phoenix-ide")));

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
}
