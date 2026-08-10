//! Logging configuration and initialization.
//!
//! Phoenix resolves three independent logging destinations from the environment:
//!
//! - `PHOENIX_LOG_STDOUT` (bool, default `true`) — structured JSON to stdout.
//! - `PHOENIX_LOG_FILE` (optional path) — structured JSON appended to that file,
//!   written by the process itself via a non-blocking worker thread. The active
//!   file keeps that exact path; Phoenix rotates it daily, compresses closed
//!   archives, and retains the newest 14 generations.
//! - `PHOENIX_FATAL_LOG_FILE` (optional path) — the latest fatal startup/runtime
//!   diagnostic. Each event overwrites the previous one and is capped at 64 KiB.
//!
//! stdout and the structured file may be enabled together; the fatal snapshot
//! records only the latest startup/runtime failure. [`LogConfig`] is resolved
//! once and is the single source of truth for both the wiring and what
//! `GET /api/deployment` reports (via [`PreparedLogConfig::to_log_info`]).

use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanLimits};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::NamedTempFile;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

mod rotation;

const FATAL_LOG_ENV: &str = "PHOENIX_FATAL_LOG_FILE";
const MAX_FATAL_LOG_BYTES: usize = 64 * 1024;
static PROCESS_LOG_CONFIG: OnceLock<LogConfig> = OnceLock::new();

pub(crate) fn process_log_config() -> &'static LogConfig {
    PROCESS_LOG_CONFIG.get_or_init(LogConfig::from_env)
}

pub(crate) fn install_fatal_diagnostic_hook() {
    process_log_config();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_fatal_diagnostic(&format!("panic: {info}"));
        previous_hook(info);
    }));
}

pub(crate) fn record_fatal_diagnostic(message: &(impl std::fmt::Display + ?Sized)) {
    let config = process_log_config();
    let Some(path) = config.fatal_file.as_deref() else {
        return;
    };
    if let Some(structured) = config.file.as_deref() {
        match paths_alias(structured, path) {
            Ok(true) => {
                eprintln!("{FATAL_LOG_ENV} must differ from PHOENIX_LOG_FILE");
                return;
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("failed to resolve fatal diagnostic path: {error}");
                return;
            }
        }
    }
    if let Err(error) = write_fatal_diagnostic(path, &message.to_string()) {
        eprintln!("failed to write {}: {error}", path.display());
    }
}

fn fatal_log_path() -> Option<PathBuf> {
    std::env::var_os(FATAL_LOG_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn paths_alias(left: &std::path::Path, right: &std::path::Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    if let (Ok(left_metadata), Ok(right_metadata)) =
        (std::fs::metadata(left), std::fs::metadata(right))
    {
        use std::os::unix::fs::MetadataExt;
        if left_metadata.dev() == right_metadata.dev()
            && left_metadata.ino() == right_metadata.ino()
        {
            return Ok(true);
        }
    }
    Ok(path_identity(left)? == path_identity(right)?)
}

fn path_identity(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other @ (std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_)) => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "log path has no existing ancestor",
            )
        })?;
        missing.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "log path has no existing ancestor",
            )
        })?;
    }
    let mut identity = std::fs::canonicalize(existing)?;
    for component in missing.iter().rev() {
        identity.push(component);
    }
    Ok(identity)
}

fn write_fatal_diagnostic(path: &std::path::Path, message: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut diagnostic = format!("fatal: {message}\n");
    if diagnostic.len() > MAX_FATAL_LOG_BYTES {
        let mut end = MAX_FATAL_LOG_BYTES;
        while !diagnostic.is_char_boundary(end) {
            end -= 1;
        }
        diagnostic.truncate(end);
    }
    let mut temporary = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(diagnostic.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn reject_symlink(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("log path must not be a symlink: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn open_log_append(path: &Path, create_mode: Option<u32>) -> std::io::Result<File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(create_mode.unwrap_or(0o600))
            .custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn prepare_log_identity(path: &Path, private: bool) -> std::io::Result<()> {
    let existed = path.exists();
    let file = open_log_append(path, Some(0o600))?;
    #[cfg(unix)]
    if private || !existed {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The resolved logging destinations.
#[derive(Clone, Debug)]
pub struct LogConfig {
    /// Whether structured logs are written to stdout.
    pub stdout: bool,
    /// Path of the process-owned log file, when file logging is enabled.
    pub file: Option<PathBuf>,
    /// Path of the bounded latest-fatal diagnostic, when configured.
    pub fatal_file: Option<PathBuf>,
}

/// Logging destinations whose paths were opened, checked for aliases, and
/// proven writable before subscriber construction.
#[derive(Clone, Debug)]
pub struct PreparedLogConfig {
    stdout: bool,
    file: Option<PathBuf>,
    fatal_file: Option<PathBuf>,
}

/// Handles that must outlive the process so background workers flush on shutdown.
///
/// - `_log_guard` — the non-blocking file appender worker (when file logging is on).
/// - `tracer_provider` — the active `OTel` tracer provider, held so spans flush
///   before exit. Call [`TracingHandles::shutdown_tracer`] during graceful
///   shutdown.
pub struct TracingHandles {
    _log_guard: Option<WorkerGuard>,
    _archive_guard: Option<rotation::ArchiveWorkerGuard>,
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
            fatal_file: fatal_log_path(),
        }
    }

    pub fn prepare(self) -> std::io::Result<PreparedLogConfig> {
        if let (Some(file), Some(fatal)) = (self.file.as_deref(), self.fatal_file.as_deref()) {
            if paths_alias(file, fatal)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{FATAL_LOG_ENV} must differ from PHOENIX_LOG_FILE"),
                ));
            }
        }
        if let Some(file) = self.file.as_deref() {
            prepare_log_identity(file, false).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to prepare PHOENIX_LOG_FILE {}: {error}",
                        file.display()
                    ),
                )
            })?;
        }
        if let Some(fatal) = self.fatal_file.as_deref() {
            prepare_log_identity(fatal, true).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to prepare {FATAL_LOG_ENV} {}: {error}",
                        fatal.display()
                    ),
                )
            })?;
        }
        if let (Some(file), Some(fatal)) = (self.file.as_deref(), self.fatal_file.as_deref()) {
            if paths_alias(file, fatal)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{FATAL_LOG_ENV} must differ from PHOENIX_LOG_FILE"),
                ));
            }
        }
        Ok(PreparedLogConfig {
            stdout: self.stdout,
            file: self.file,
            fatal_file: self.fatal_file,
        })
    }
}

impl PreparedLogConfig {
    /// The deployment-report view of the active sinks, with the file path made
    /// absolute. Derived from the same prepared values used by the subscriber.
    pub fn to_log_info(&self) -> crate::api::LogInfo {
        crate::api::LogInfo {
            stdout: self.stdout,
            file: self
                .file
                .as_deref()
                .map(|p| crate::api::absolutize(p).display().to_string()),
            fatal_file: self
                .fatal_file
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
pub fn init(config: &PreparedLogConfig) -> std::io::Result<TracingHandles> {
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

    let (file_layer, guard, archive_guard) = match config.file.as_deref() {
        Some(path) => {
            let (file, archive_guard) = rotation::DailyRotatingFile::open(path).map_err(|e| {
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
            (Some(layer), Some(guard), Some(archive_guard))
        }
        None => (None, None, None),
    };

    tracing_subscriber::registry()
        .with(otel_layer)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    if let Some(archive_guard) = archive_guard.as_ref() {
        archive_guard.start_maintenance()?;
    }
    install_panic_hook();
    Ok(TracingHandles {
        _log_guard: guard,
        _archive_guard: archive_guard,
        tracer_provider,
    })
}

const OTEL_SPANS: &[(&str, &str)] = &[
    ("phoenix_ide::otel", "http"),
    ("phoenix_ide::otel", "pr_status.refresh"),
    ("phoenix_ide::otel", "conversation.stream.init"),
    ("phoenix_ide::otel", "browser.conversation_open"),
    ("phoenix_ide::otel", "conversation.runtime.materialize"),
    ("phoenix_ide::otel", "conversation.cancel"),
    ("phoenix_ide::otel", "conversation.turn"),
    ("phoenix_ide::otel", "direct_turn.settle"),
    ("phoenix_ide::otel", "tool.execute"),
    ("phoenix_llm::otel", "llm.request"),
];

pub(crate) fn conversation_cancel_span(conversation_id: &str) -> tracing::Span {
    tracing::info_span!(
        target: "phoenix_ide::otel",
        "conversation.cancel",
        conv_id = %conversation_id,
        observed_state = tracing::field::Empty,
        outcome = tracing::field::Empty,
        direct_turn_action = tracing::field::Empty,
        turn_id = tracing::field::Empty,
        generation = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    )
}

pub(crate) fn direct_turn_settlement_span(
    parent: &tracing::Span,
    conversation_id: &str,
    turn_id: u64,
    generation: u64,
    terminal_kind: &str,
    target_state: &str,
) -> tracing::Span {
    direct_turn_settlement_span_for_path(
        parent,
        conversation_id,
        turn_id,
        generation,
        terminal_kind,
        target_state,
        "state",
    )
}

pub(crate) fn continuation_direct_turn_settlement_span(
    parent: &tracing::Span,
    conversation_id: &str,
    turn_id: u64,
    generation: u64,
    terminal_kind: &str,
    target_state: &str,
    operation_id: &str,
) -> tracing::Span {
    let span = direct_turn_settlement_span_for_path(
        parent,
        conversation_id,
        turn_id,
        generation,
        terminal_kind,
        target_state,
        "continuation",
    );
    span.record("operation_id", operation_id);
    span
}

fn direct_turn_settlement_span_for_path(
    parent: &tracing::Span,
    conversation_id: &str,
    turn_id: u64,
    generation: u64,
    terminal_kind: &str,
    target_state: &str,
    settlement_path: &str,
) -> tracing::Span {
    tracing::info_span!(
        target: "phoenix_ide::otel",
        parent: parent,
        "direct_turn.settle",
        conv_id = %conversation_id,
        turn_id,
        generation,
        terminal_kind,
        target_state,
        settlement_path,
        operation_id = tracing::field::Empty,
        outcome = tracing::field::Empty,
        commit_probe = tracing::field::Empty,
        durable_state = tracing::field::Empty,
        active_turn_present = tracing::field::Empty,
        turn_still_active = tracing::field::Empty,
        error.message = tracing::field::Empty,
        probe.error.message = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    )
}

fn otel_metadata_enabled(meta: &tracing::Metadata<'_>) -> bool {
    meta.is_span() && OTEL_SPANS.contains(&(meta.target(), meta.name()))
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

    #[allow(clippy::too_many_lines)]
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
            let http = tracing::info_span!(target: "phoenix_ide::otel", "http");
            let pr_refresh = tracing::info_span!(
                target: "phoenix_ide::otel",
                parent: &http,
                "pr_status.refresh",
                operation = "branch_and_work_change",
            );
            drop(pr_refresh);
            let cancel = conversation_cancel_span("cancel-conversation-123");
            cancel.record("observed_state", "LlmRequesting");
            cancel.record("outcome", "runtime_cancel_requested");
            cancel.record("direct_turn_action", "not_checked");
            drop(cancel);
            drop(http);
            let stream_init = tracing::info_span!(
                target: "phoenix_ide::otel",
                "conversation.stream.init",
                stream.time_to_init_ms = 42_u64,
            );
            let browser_open = tracing::info_span!(
                target: "phoenix_ide::otel",
                "browser.conversation_open",
                browser.total_ms = 120_u64,
            );
            let runtime_materialize = tracing::info_span!(
                target: "phoenix_ide::otel",
                "conversation.runtime.materialize",
                runtime.materialization_ms = 84_u64,
            );
            drop(stream_init);
            drop(runtime_materialize);
            drop(browser_open);
            let turn = tracing::info_span!(
                target: "phoenix_ide::otel",
                "conversation.turn",
                conv_id = "settlement-conversation-123",
            );
            let settlement = direct_turn_settlement_span(
                &turn,
                "settlement-conversation-123",
                265,
                0,
                "completed",
                "Idle",
            );
            settlement.record("outcome", "failed_still_owed");
            settlement.record("commit_probe", "still_owed");
            settlement.record("durable_state", "LlmRequesting");
            settlement.record("active_turn_present", true);
            settlement.record("turn_still_active", true);
            settlement.record("error.message", "database is locked");
            settlement.record("otel.status_code", "ERROR");
            drop(settlement);
            let continuation_settlement = continuation_direct_turn_settlement_span(
                &turn,
                "continuation-settlement-conversation-123",
                266,
                1,
                "failed",
                "ContextExhausted",
                "continuation-operation-123",
            );
            continuation_settlement.record("outcome", "reconciled_duplicate");
            continuation_settlement.record("commit_probe", "retry");
            drop(continuation_settlement);
            drop(turn);
            let llm = tracing::info_span!(
                target: "phoenix_llm::otel",
                "llm.request",
                model = "gpt-test",
                provider = "openai",
                transport = "http_sse",
                request_id = "request-123",
                conv_id = "conversation-123",
                retry_attempt = 2_u64,
                stream.first_generation_event_ms = tracing::field::Empty,
            );
            llm.record("stream.first_generation_event_ms", 1_234_i64);
            let _guard = llm.enter();
            tracing::debug!(target: "tokio_tungstenite", frame = "PAYLOAD_SENTINEL", "frame");
            tracing::info!(delta = "DELTA_SENTINEL", "response delta");
            let dependency =
                tracing::debug_span!(target: "sqlx::query", "query", sql = "SELECT secret");
            drop(dependency);
            let http_collision = tracing::info_span!(
                target: "reqwest::otel",
                "http",
                authorization = "Bearer HTTP_COLLISION_SENTINEL"
            );
            let turn_collision = tracing::info_span!(
                target: "foreign_runtime",
                "conversation.turn",
                payload = "TURN_COLLISION_SENTINEL"
            );
            let llm_collision = tracing::info_span!(
                target: "foreign_llm",
                "llm.request",
                payload = "LLM_COLLISION_SENTINEL"
            );
            let tool_collision = tracing::info_span!(
                target: "foreign_tools",
                "tool.execute",
                payload = "TOOL_COLLISION_SENTINEL"
            );
            drop((
                http_collision,
                turn_collision,
                llm_collision,
                tool_collision,
            ));
        });
        provider.force_flush().expect("flush spans");

        let spans = exporter.get_finished_spans().expect("exported spans");
        assert_eq!(spans.len(), 10);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.name.as_ref())
                .collect::<Vec<_>>(),
            [
                "pr_status.refresh",
                "conversation.cancel",
                "http",
                "conversation.stream.init",
                "conversation.runtime.materialize",
                "browser.conversation_open",
                "direct_turn.settle",
                "direct_turn.settle",
                "conversation.turn",
                "llm.request"
            ]
        );
        assert!(spans.iter().all(|span| span.events.is_empty()));
        let llm_span = spans
            .iter()
            .find(|span| span.name == "llm.request")
            .expect("LLM span exported");
        let attributes = format!("{:?}", llm_span.attributes);
        let ttft = llm_span
            .attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "stream.first_generation_event_ms")
            .expect("numeric TTFT attribute exported");
        assert!(
            matches!(ttft.value, opentelemetry::Value::I64(1_234)),
            "TTFT must remain numeric for TraceQL comparisons: {:?}",
            ttft.value
        );
        for required in [
            "gpt-test",
            "openai",
            "http_sse",
            "request-123",
            "conversation-123",
        ] {
            assert!(
                attributes.contains(required),
                "missing attribute {required}"
            );
        }
        let encoded = format!("{spans:?}");
        for required in [
            "cancel-conversation-123",
            "settlement-conversation-123",
            "failed_still_owed",
            "still_owed",
            "database is locked",
            "state",
            "continuation",
            "continuation-operation-123",
            "reconciled_duplicate",
        ] {
            assert!(
                encoded.contains(required),
                "missing exported telemetry attribute {required}"
            );
        }
        for forbidden in [
            "PAYLOAD_SENTINEL",
            "DELTA_SENTINEL",
            "SELECT secret",
            "authorization",
            "HTTP_COLLISION_SENTINEL",
            "TURN_COLLISION_SENTINEL",
            "LLM_COLLISION_SENTINEL",
            "TOOL_COLLISION_SENTINEL",
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
    fn fatal_diagnostic_overwrites_and_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fatal.log");

        write_fatal_diagnostic(&path, "first failure").unwrap();
        write_fatal_diagnostic(&path, "second failure").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fatal: second failure\n"
        );

        write_fatal_diagnostic(&path, &"é".repeat(MAX_FATAL_LOG_BYTES)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() <= MAX_FATAL_LOG_BYTES);
        assert!(std::str::from_utf8(&bytes).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fatal_diagnostic_replaces_symlink_without_touching_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let path = directory.path().join("fatal.log");
        std::fs::write(&target, "keep me").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        write_fatal_diagnostic(&path, "failure").unwrap();

        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fatal: failure\n");
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn log_path_identity_rejects_parent_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("logs");
        std::fs::create_dir(&nested).unwrap();

        assert!(paths_alias(
            &nested.join("../prod.log"),
            &directory.path().join("prod.log"),
        )
        .unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn log_path_identity_rejects_symlink_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let link = directory.path().join("alias");
        std::os::unix::fs::symlink(directory.path(), &link).unwrap();

        assert!(paths_alias(&link.join("prod.log"), &directory.path().join("prod.log"),).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn log_path_identity_rejects_hard_link_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("prod.log");
        let alias = directory.path().join("fatal.log");
        std::fs::write(&original, "log").unwrap();
        std::fs::hard_link(&original, &alias).unwrap();

        assert!(paths_alias(&original, &alias).unwrap());
    }

    #[test]
    fn stdout_enabled_by_truthy_values() {
        for v in ["1", "true", "on", "yes", "anything"] {
            assert!(stdout_enabled(Some(v)), "{v:?} should enable stdout");
        }
    }

    #[test]
    fn to_log_info_absolutizes_the_file_and_carries_stdout() {
        let cfg = PreparedLogConfig {
            stdout: false,
            file: Some(PathBuf::from("relative.log")),
            fatal_file: Some(PathBuf::from("relative-fatal.log")),
        };
        let info = cfg.to_log_info();
        assert!(!info.stdout);
        let file = info.file.expect("file sink");
        assert!(std::path::Path::new(&file).is_absolute());
        assert!(file.ends_with("relative.log"));
        assert!(info
            .fatal_file
            .expect("fatal diagnostic")
            .ends_with("relative-fatal.log"));
    }

    #[test]
    fn rotating_file_errors_when_parent_is_not_a_directory() {
        // A requested file under a path whose parent is a regular file cannot be
        // created; init turns this Err into a startup abort (fail fast).
        let blocker =
            std::env::temp_dir().join(format!("phoenix-logging-test-{}", std::process::id()));
        std::fs::write(&blocker, b"x").unwrap();
        let unopenable = blocker.join("nested.log");
        assert!(rotation::DailyRotatingFile::open(&unopenable).is_err());
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn fatal_only_configuration_is_prepared() {
        let directory = tempfile::tempdir().unwrap();
        let blocker = directory.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let config = LogConfig {
            stdout: false,
            file: None,
            fatal_file: Some(blocker.join("fatal.log")),
        };

        let error = config.prepare().unwrap_err();

        assert!(error.to_string().contains(FATAL_LOG_ENV));
    }

    #[cfg(unix)]
    #[test]
    fn configured_log_symlink_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.log");
        let link = directory.path().join("prod.log");
        std::fs::write(&target, "existing").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let config = LogConfig {
            stdout: false,
            file: Some(link),
            fatal_file: None,
        };

        let error = config.prepare().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "existing");
    }

    #[test]
    fn to_log_info_reports_no_file_when_unset() {
        let cfg = PreparedLogConfig {
            stdout: true,
            file: None,
            fatal_file: None,
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
