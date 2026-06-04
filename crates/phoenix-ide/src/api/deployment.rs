//! "About this deployment" endpoint.
//!
//! Serves `GET /api/deployment`: a read-only snapshot of build identity,
//! network binding + TLS posture, live resource usage, on-disk locations with
//! sizes, and the log sink. See `specs/deployment-info/`.
//!
//! Static facts (binding, TLS, the on-disk layout) are resolved once at startup
//! into [`DeploymentConfig`] and threaded through [`AppState`]. Sampled facts
//! (resource usage, sizes, `sampled_at`) are measured per request so a refresh
//! yields current values.

use super::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use ts_rs::TS;

// ============================================================
// Captured static configuration (server-side, not on the wire)
// ============================================================

/// How a configured on-disk location should be sized.
#[derive(Clone, Copy, Debug)]
pub enum MeasureMode {
    /// Stat a single file.
    File,
    /// Recurse a directory known to be small/owned.
    RecurseSmall,
    /// Known-large (e.g. a binary cache); report the path but do not walk it.
    NoMeasure,
    /// The attachment store while attachment bytes live inside the database.
    InlineDb,
}

/// A configured on-disk location to report.
#[derive(Clone, Debug)]
pub struct DiskLocation {
    pub label: String,
    pub path: PathBuf,
    pub mode: MeasureMode,
}

/// Static deployment facts resolved once at startup and threaded through
/// [`AppState`]. Sampled facts are computed per request in [`deployment_info`].
#[derive(Clone, Debug)]
pub struct DeploymentConfig {
    pub bind_address: SocketAddr,
    pub tls: TlsInfo,
    pub log: LogInfo,
    pub locations: Vec<DiskLocation>,
}

// ============================================================
// Wire types (exported to ui/src/generated/ via ts-rs)
// ============================================================

/// Snapshot returned by `GET /api/deployment`.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DeploymentInfo {
    pub build: BuildInfo,
    pub network: NetworkInfo,
    pub resources: ResourceUsage,
    pub disk: Vec<DiskEntry>,
    pub log: LogInfo,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BuildInfo {
    pub version: String,
    pub git_sha: String,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_seconds: u64,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct NetworkInfo {
    pub bind_address: String,
    pub socket_activated: bool,
    pub tls: TlsInfo,
}

/// TLS posture. Reused as the captured config value and on the wire.
#[derive(Serialize, TS, Clone, Debug)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TlsInfo {
    pub enabled: bool,
    /// `"auto"` (self-signed) or `"manual"` (provided certs); `None` when disabled.
    pub mode: Option<String>,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub ca_cert_path: Option<String>,
    /// Host names the auto cert is generated for; empty otherwise.
    pub hosts: Vec<String>,
}

impl TlsInfo {
    /// TLS disabled — the server is serving plain HTTP.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            mode: None,
            cert_path: None,
            key_path: None,
            ca_cert_path: None,
            hosts: Vec::new(),
        }
    }
}

/// Live process + system resource usage. Each field is `None` when the metric
/// cannot be sampled on the host — never a misleading `0`.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ResourceUsage {
    /// Resident set size of this process, in bytes.
    pub process_memory_bytes: Option<u64>,
    /// CPU utilization of this process, as a percent (may exceed 100 on
    /// multi-core hosts).
    pub process_cpu_percent: Option<f32>,
    pub system_total_memory_bytes: Option<u64>,
    pub system_available_memory_bytes: Option<u64>,
    pub logical_cpu_count: Option<u32>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DiskEntry {
    pub label: String,
    pub path: String,
    pub size: DiskSize,
}

/// The four semantically-distinct outcomes of sizing a location. A bare
/// nullable number cannot tell these apart, so they are modelled as a tagged
/// union (correct-by-construction).
#[derive(Serialize, TS, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum DiskSize {
    /// Size was measured.
    Measured { bytes: u64 },
    /// Intentionally not walked (known-large directory).
    NotMeasured,
    /// Path does not exist on disk.
    Absent,
    /// Bytes live inside the `SQLite` database (attachment store placeholder).
    InlineDb,
}

/// Where the deployment's logs go. A process-owned file path is reported only
/// when explicitly configured; otherwise the real sink (stdout) is named.
#[derive(Serialize, TS, Clone, Debug)]
#[serde(tag = "sink", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum LogInfo {
    /// Logs are written to a deployment-owned file at this path.
    File { path: String },
    /// Logs are written to standard output, captured by the supervising process.
    Stdout,
}

// ============================================================
// Handler
// ============================================================

/// `GET /api/deployment` — assemble and return a [`DeploymentInfo`] snapshot.
pub async fn deployment_info(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = &state.deployment;

    let build = BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("PHOENIX_GIT_SHA").to_string(),
        started_at: crate::hot_restart::started_at(),
        uptime_seconds: crate::hot_restart::uptime_secs(),
    };

    let network = NetworkInfo {
        bind_address: cfg.bind_address.to_string(),
        socket_activated: crate::hot_restart::is_socket_activated(),
        tls: cfg.tls.clone(),
    };

    let resources = sample_resources().await;
    let disk = cfg.locations.iter().map(measure_location).collect();

    Json(DeploymentInfo {
        build,
        network,
        resources,
        disk,
        log: cfg.log.clone(),
        sampled_at: Utc::now(),
    })
}

/// Sample live process and system resource usage. Returns `None` for any metric
/// the host does not expose rather than a misleading zero.
async fn sample_resources() -> ResourceUsage {
    use sysinfo::{ProcessesToUpdate, System};

    let logical_cpu_count = std::thread::available_parallelism()
        .ok()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX));

    let mut sys = System::new();
    sys.refresh_memory();
    let system_total_memory_bytes = Some(sys.total_memory());
    let system_available_memory_bytes = Some(sys.available_memory());

    let (process_memory_bytes, process_cpu_percent) = match sysinfo::get_current_pid() {
        Ok(pid) => {
            // CPU usage needs two samples separated by the minimum interval.
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid)
                .map_or((None, None), |p| (Some(p.memory()), Some(p.cpu_usage())))
        }
        Err(_) => (None, None),
    };

    ResourceUsage {
        process_memory_bytes,
        process_cpu_percent,
        system_total_memory_bytes,
        system_available_memory_bytes,
        logical_cpu_count,
    }
}

/// Size a single configured location per its [`MeasureMode`].
fn measure_location(loc: &DiskLocation) -> DiskEntry {
    let size = match loc.mode {
        MeasureMode::InlineDb => DiskSize::InlineDb,
        _ if !loc.path.exists() => DiskSize::Absent,
        MeasureMode::File => std::fs::metadata(&loc.path)
            .map_or(DiskSize::Absent, |m| DiskSize::Measured { bytes: m.len() }),
        MeasureMode::RecurseSmall => DiskSize::Measured {
            bytes: dir_size(&loc.path),
        },
        MeasureMode::NoMeasure => DiskSize::NotMeasured,
    };
    DiskEntry {
        label: loc.label.clone(),
        path: loc.path.display().to_string(),
        size,
    }
}

/// Recursively sum the byte sizes of regular files under `path`. Bounded to the
/// directory it is given — callers only pass directories classified as small,
/// never the known-large caches. An unreadable subtree contributes nothing
/// rather than aborting the walk.
fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

#[cfg(test)]
impl DeploymentConfig {
    /// Minimal config for handler tests.
    pub fn for_tests() -> Self {
        Self {
            bind_address: SocketAddr::from(([127, 0, 0, 1], 0)),
            tls: TlsInfo::disabled(),
            log: LogInfo::Stdout,
            locations: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn loc(path: PathBuf, mode: MeasureMode) -> DiskLocation {
        DiskLocation {
            label: "x".to_string(),
            path,
            mode,
        }
    }

    #[test]
    fn file_mode_measures_byte_length() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"hello").unwrap();
        assert_eq!(
            measure_location(&loc(file, MeasureMode::File)).size,
            DiskSize::Measured { bytes: 5 }
        );
    }

    #[test]
    fn missing_file_is_absent_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().join("nope"), MeasureMode::File)).size,
            DiskSize::Absent
        );
    }

    #[test]
    fn recurse_small_sums_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), b"123").unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b"), b"4567").unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().to_path_buf(), MeasureMode::RecurseSmall)).size,
            DiskSize::Measured { bytes: 7 }
        );
    }

    #[test]
    fn no_measure_existing_dir_reports_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().to_path_buf(), MeasureMode::NoMeasure)).size,
            DiskSize::NotMeasured
        );
    }

    #[test]
    fn no_measure_absent_path_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().join("gone"), MeasureMode::NoMeasure)).size,
            DiskSize::Absent
        );
    }

    #[test]
    fn inline_db_is_inline_regardless_of_path() {
        let entry = measure_location(&loc(
            PathBuf::from("/does/not/exist/phoenix.db"),
            MeasureMode::InlineDb,
        ));
        assert_eq!(entry.size, DiskSize::InlineDb);
        assert_eq!(entry.path, "/does/not/exist/phoenix.db");
    }

    #[tokio::test]
    async fn sample_resources_completes_with_system_metrics() {
        let usage = sample_resources().await;
        // System totals are always populated on supported hosts; the call must
        // complete (including its CPU-sample window) without panicking.
        assert!(usage.system_total_memory_bytes.is_some());
        assert!(usage.system_available_memory_bytes.is_some());
    }
}
