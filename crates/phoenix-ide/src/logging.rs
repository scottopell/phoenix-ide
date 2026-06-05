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

use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// The resolved set of enabled log sinks.
pub struct LogConfig {
    /// Whether structured logs are written to stdout.
    pub stdout: bool,
    /// Path of the process-owned log file, when file logging is enabled.
    pub file: Option<PathBuf>,
}

impl LogConfig {
    /// Resolve the sink configuration from the environment.
    pub fn from_env() -> Self {
        // stdout defaults on; only explicit falsey values disable it.
        let stdout = std::env::var("PHOENIX_LOG_STDOUT").map_or(true, |v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no" | ""
            )
        });
        let file = std::env::var_os("PHOENIX_LOG_FILE")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());
        Self { stdout, file }
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
/// Returns the [`WorkerGuard`] for the file appender (when file logging is
/// enabled); the caller MUST hold it for the process lifetime so buffered log
/// lines are flushed on shutdown. Returns `None` when no file sink is active.
#[must_use]
pub fn init(config: &LogConfig) -> Option<WorkerGuard> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "phoenix_ide=debug,tower_http=debug".into());

    let stdout_layer = config.stdout.then(|| {
        fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(false)
    });

    let (file_layer, guard) = match config.file.as_deref() {
        Some(path) => match open_append(path) {
            Ok(file) => {
                let (writer, guard) = tracing_appender::non_blocking(file);
                let layer = fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(false)
                    .with_writer(writer);
                (Some(layer), Some(guard))
            }
            Err(e) => {
                // The subscriber isn't up yet, so this is the one place a plain
                // stderr write is the right call.
                eprintln!(
                    "phoenix-ide: failed to open PHOENIX_LOG_FILE {}: {e}",
                    path.display()
                );
                (None, None)
            }
        },
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    install_panic_hook();
    guard
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
