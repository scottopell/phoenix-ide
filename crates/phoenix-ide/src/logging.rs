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
/// Returns the [`WorkerGuard`] for the file appender (when file logging is
/// enabled); the caller MUST hold it for the process lifetime so buffered log
/// lines are flushed on shutdown. Returns `Ok(None)` when no file sink is
/// active.
///
/// Fails (aborting startup) when `PHOENIX_LOG_FILE` is set but cannot be
/// opened. Honoring the configured sinks exactly — or refusing to start — is
/// what lets the deployment report derive its file path from [`LogConfig`]
/// without ever advertising a sink the subscriber isn't writing (REQ-DEPLOY-006).
pub fn init(config: &LogConfig) -> std::io::Result<Option<WorkerGuard>> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "phoenix_ide=debug,tower_http=debug".into());

    let stdout_layer = config.stdout.then(|| {
        fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(false)
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
                .with_writer(writer);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    install_panic_hook();
    Ok(guard)
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
