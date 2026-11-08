use super::{FailureCode, Handle, MemorySource, Observation, Phase, ProcessIdentity, Sample};
use crate::api::AppState;
use crate::operational_observations::FailureBoundary;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use std::time::Duration;

const CADENCE: Duration = Duration::from_secs(60);

fn unix_ms() -> Option<u64> {
    u64::try_from(chrono::Utc::now().timestamp_millis()).ok()
}

fn unavailable(phase: Phase) -> Sample {
    Sample {
        phase,
        sampled_unix_ms: unix_ms(),
        process_uptime_ms: crate::hot_restart::uptime_secs().saturating_mul(1000),
        host_uptime_secs: sysinfo::IS_SUPPORTED_SYSTEM
            .then(sysinfo::System::uptime)
            .filter(|uptime| *uptime != 0),
        generation_unix_ms: None,
        disk_unix_ms: None,
        memory_source: if cfg!(target_os = "linux") {
            MemorySource::LinuxPss
        } else if cfg!(target_os = "macos") {
            MemorySource::MacPhysicalFootprint
        } else {
            MemorySource::Unavailable
        },
        current_rss_bytes: None,
        proportional_bytes: None,
        lifetime_max_rss_bytes: None,
        host_bytes: [None; 4],
        host_pressure: None,
        disk_available_bytes: [None; 3],
        managed_proportional_bytes: None,
        managed_pid_count: None,
        live_materialized_handles: None,
        last_failure_unix_ms: None,
        sqlite_extended_code: None,
    }
}

pub(crate) fn initialize(environment: &PhoenixRuntimeEnvironment) -> Option<Handle> {
    if std::fs::create_dir_all(environment.data_dir()).is_err() {
        tracing::warn!(
            code = "recorder_init",
            "resource recorder unavailable; startup continues"
        );
        return None;
    }
    let process = ProcessIdentity {
        pid: std::process::id(),
        started_unix_ms: crate::hot_restart::started_at()
            .and_then(|time| u64::try_from(time.timestamp_millis()).ok()),
    };
    tracing::debug!(
        code = "recorder_metric_gap",
        "lifetime maximum RSS, host pressure and SQLite codes have no supported recorder source"
    );
    if let Ok(handle) = super::start(environment.data_dir(), &process) {
        let mut sample = unavailable(Phase::Startup);
        let failure_counts = failures(&mut sample);
        handle.try_record(&Observation {
            sample,
            failure_counts,
        });
        Some(handle)
    } else {
        tracing::warn!(
            code = "recorder_init",
            "resource recorder unavailable; startup continues"
        );
        None
    }
}

fn failures(sample: &mut Sample) -> [Option<u64>; 11] {
    let mut counts = [None; 11];
    for failure in crate::operational_observations::snapshot() {
        let index = match failure.boundary {
            FailureBoundary::OrdinaryToolSettlement => 0,
            FailureBoundary::StartupRequiredAuthority => 1,
            FailureBoundary::SettlementOwnerDisappeared => 2,
            FailureBoundary::OtherLocalAuthority => 3,
            FailureBoundary::StructuredLogWrite => 4,
            FailureBoundary::StructuredLogFlush => 5,
            FailureBoundary::StdoutWrite => 6,
            FailureBoundary::StdoutFlush => 7,
            FailureBoundary::FatalDiagnosticWrite => 8,
            FailureBoundary::TraceExport => 9,
        };
        counts[index] = Some(failure.count);
        if let Some(time) = failure.last_observed_at {
            sample.last_failure_unix_ms = sample
                .last_failure_unix_ms
                .max(u64::try_from(time.timestamp_millis()).ok());
        }
    }
    counts
}

fn stopped(handle: &Handle) -> bool {
    let snapshot = handle.snapshot();
    if snapshot.failure == FailureCode::None {
        return false;
    }
    tracing::debug!(
        code = ?snapshot.failure,
        sampled = snapshot.sampled,
        queued = snapshot.queued,
        appended = snapshot.appended,
        committed = snapshot.committed,
        last_committed_sequence = snapshot.last_committed_sequence,
        dropped_full = snapshot.dropped_full,
        dropped_disconnected = snapshot.dropped_disconnected,
        dropped_encode = snapshot.dropped_encode,
        dropped_oversized = snapshot.dropped_oversized,
        io_errors = snapshot.io_errors,
        "resource recorder stopped; no sampling or storage retry"
    );
    true
}

async fn next_attempt(handle: &Handle) -> bool {
    tokio::time::sleep(CADENCE).await;
    !stopped(handle)
}

pub(crate) fn spawn_sampler(state: AppState, handle: Handle) {
    tokio::spawn(async move {
        while next_attempt(&handle).await {
            let generation = state.resource_monitor.observe(&state).await;
            let summary = crate::api::deployment::resource_summary(&generation);
            let mut sample = unavailable(Phase::Running);
            sample.generation_unix_ms =
                u64::try_from(generation.sampled_at.timestamp_millis()).ok();
            sample.current_rss_bytes = summary.api_rss_bytes;
            sample.proportional_bytes = summary.api_proportional_bytes;
            sample.managed_proportional_bytes = summary.managed_total.memory_bytes;
            sample.managed_pid_count =
                Some(u64::from(summary.managed_total.deduplicated_pid_count));
            sample.host_bytes = [
                generation.host.total_memory_bytes,
                generation.host.available_memory_bytes,
                generation.host_swap[0],
                generation.host_swap[1],
            ];
            sample.disk_available_bytes =
                crate::api::deployment_observations::recorder_space(&state.runtime_env).await;
            sample.disk_unix_ms = unix_ms();
            sample.live_materialized_handles = state.runtime.live_materialized_handle_count();
            let mut failure_counts = failures(&mut sample);
            let sinks = crate::logging::observations::snapshot(&state.deployment.log);
            failure_counts[10] = sinks.structured_queue_drops;
            if !sinks.structured_file_configured {
                failure_counts[4] = None;
                failure_counts[5] = None;
            }
            if !sinks.stdout_configured {
                failure_counts[6] = None;
                failure_counts[7] = None;
            }
            if !sinks.fatal_diagnostic_configured {
                failure_counts[8] = None;
            }
            if !matches!(
                sinks.trace_failure_coverage,
                crate::logging::observations::TraceFailureCoverage::OtlpExportResults
            ) {
                failure_counts[9] = None;
            }
            sample.sampled_unix_ms = unix_ms();
            sample.process_uptime_ms = crate::hot_restart::uptime_secs().saturating_mul(1000);
            handle.try_record(&Observation {
                sample,
                failure_counts,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_prepares_typed_data_parent_before_recorder_and_required_database() {
        let source = include_str!("sampling.rs");
        let initialize = source
            .split("pub(crate) fn initialize(")
            .nth(1)
            .unwrap()
            .split("fn failures(")
            .next()
            .unwrap();
        let prepare = initialize
            .find("std::fs::create_dir_all(environment.data_dir())")
            .unwrap();
        let start = initialize
            .find("super::start(environment.data_dir(), &process)")
            .unwrap();
        assert!(prepare < start);
        assert!(initialize.contains("return None;"));
        assert!(!initialize.contains("set_permissions"));
        let server = include_str!("../lib.rs");
        let server = server.split("pub async fn run_server()").nth(1).unwrap();
        assert!(
            server
                .find("resource_flight_recorder::initialize(&runtime_env)")
                .unwrap()
                < server
                    .find("open_database_with_migrations(&runtime_env).await?")
                    .unwrap()
        );
    }

    #[test]
    fn unavailable_is_null_and_not_a_peak_or_zero() {
        let sample = unavailable(Phase::Startup);
        let value = serde_json::to_value(sample).unwrap();
        for key in ["g", "d", "r", "p", "l", "v", "a", "n", "w", "e", "c"] {
            assert!(value[key].is_null(), "{key}");
        }
        assert_eq!(value["b"], serde_json::json!([null, null, null, null]));
        assert_eq!(value["s"], serde_json::json!([null, null, null]));
    }

    #[tokio::test(start_paused = true)]
    async fn cadence_waits_after_work_without_catching_up() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        let handle = Handle {
            sender,
            counters: std::sync::Arc::default(),
        };
        for _ in 0..2 {
            let next = next_attempt(&handle);
            tokio::pin!(next);
            tokio::select! {
                biased;
                _ = &mut next => panic!("sampling started before cadence"),
                () = tokio::time::sleep(Duration::from_secs(59)) => {}
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(next.await);
            tokio::time::advance(Duration::from_secs(300)).await;
        }
    }

    #[test]
    fn sampling_wait_is_at_least_a_minute_and_storage_failure_is_terminal() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        let counters = std::sync::Arc::new(super::super::Counters::default());
        let handle = Handle {
            sender,
            counters: counters.clone(),
        };
        assert!(!stopped(&handle));
        counters
            .failure
            .store(FailureCode::Io as u64, std::sync::atomic::Ordering::Relaxed);
        assert!(stopped(&handle));
    }
}
