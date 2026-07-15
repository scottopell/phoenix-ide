//! Demand-driven, request-coalesced process resource observations.

use super::deployment::HostResources;
use super::process_sample::{
    group_member_identities_for_sampling, process_identity_for_sampling,
    sample_process_observations, session_member_identities_for_sampling, ProcessIdentity,
    ProcessObservation,
};
use super::AppState;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const FRESHNESS_LEASE: Duration = Duration::from_millis(1_200);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservationKey {
    handles: Vec<(String, String, i32)>,
    terminal_sessions: Vec<i32>,
}

impl ObservationKey {
    fn from_sources(
        groups: &[phoenix_tools::bash::registry::LiveHandleProcessGroup],
        terminal_sessions: &BTreeSet<i32>,
    ) -> Self {
        let mut identities = groups
            .iter()
            .map(|group| {
                (
                    group.work_scope.stable_key(),
                    group.handle_id.to_string(),
                    group.pgid,
                )
            })
            .collect::<Vec<_>>();
        identities.sort_unstable();
        Self {
            handles: identities,
            terminal_sessions: terminal_sessions.iter().copied().collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HandleObservationTarget {
    pub scope_key: String,
    pub handle_id: String,
    pub pids: BTreeSet<u32>,
}

#[derive(Debug, Clone)]
pub struct ResourceObservationGeneration {
    pub sampled_at: DateTime<Utc>,
    pub host: HostResources,
    pub observations: BTreeMap<u32, ProcessObservation>,
    pub api_pids: BTreeSet<u32>,
    pub terminal_pids: Option<BTreeSet<u32>>,
    pub handles: Vec<HandleObservationTarget>,
    pub bash_attribution_available: bool,
}

impl ResourceObservationGeneration {
    pub fn all_bash_pids(&self) -> BTreeSet<u32> {
        self.handles
            .iter()
            .flat_map(|target| target.pids.iter().copied())
            .collect()
    }

    pub fn handle_pids(&self, scope_key: &str, handle_id: &str) -> Option<&BTreeSet<u32>> {
        self.handles
            .iter()
            .find(|target| target.scope_key == scope_key && target.handle_id == handle_id)
            .map(|target| &target.pids)
    }

    pub fn scope_pids(&self, scope_key: &str) -> BTreeSet<u32> {
        self.handles
            .iter()
            .filter(|target| target.scope_key == scope_key)
            .flat_map(|target| target.pids.iter().copied())
            .collect()
    }
}

pub fn health_for_pids(
    generation: &ResourceObservationGeneration,
    pids: &BTreeSet<u32>,
) -> phoenix_core::domain::work_scope_inventory::ResourceHealth {
    let rows: Vec<&ProcessObservation> = pids
        .iter()
        .filter_map(|pid| generation.observations.get(pid))
        .collect();
    if rows.is_empty() {
        return phoenix_core::domain::work_scope_inventory::ResourceHealth {
            cpu_percent: None,
            memory_bytes: None,
            process_count: None,
        };
    }
    let cpu_values: Vec<f32> = rows.iter().filter_map(|row| row.cpu_percent).collect();
    let memory_values: Option<Vec<u64>> = rows.iter().map(|row| row.memory_bytes).collect();
    phoenix_core::domain::work_scope_inventory::ResourceHealth {
        cpu_percent: (!cpu_values.is_empty()).then(|| cpu_values.iter().sum()),
        memory_bytes: memory_values.map(|values| values.iter().sum()),
        process_count: Some(u32::try_from(rows.len()).unwrap_or(u32::MAX)),
    }
}

#[derive(Default)]
struct MonitorState {
    cached: Option<(ObservationKey, Instant, Arc<ResourceObservationGeneration>)>,
    sample_count: u64,
}

/// A short-lived observation lease. Holding the mutex across the one underlying
/// sample makes concurrent callers join the same generation; no background task
/// exists when requests stop.
#[derive(Default)]
pub struct ResourceMonitor {
    state: Mutex<MonitorState>,
}

impl ResourceMonitor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn observe(&self, app: &AppState) -> Arc<ResourceObservationGeneration> {
        let live_groups = app
            .runtime
            .bash_handles()
            .snapshot_live_process_groups()
            .await;
        let terminal_sessions = app
            .terminals
            .snapshot_shell_session_ids()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let key = ObservationKey::from_sources(&live_groups, &terminal_sessions);
        self.observe_with(key, || sample_generation(live_groups, terminal_sessions))
            .await
    }

    async fn observe_with<F, Fut>(
        &self,
        key: ObservationKey,
        sample: F,
    ) -> Arc<ResourceObservationGeneration>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ResourceObservationGeneration>,
    {
        let mut state = self.state.lock().await;
        if let Some((cached_key, created, generation)) = &state.cached {
            if cached_key == &key && created.elapsed() <= FRESHNESS_LEASE {
                return generation.clone();
            }
        }

        let generation = Arc::new(sample().await);
        state.sample_count = state.sample_count.saturating_add(1);
        state.cached = Some((key, Instant::now(), generation.clone()));
        generation
    }

    #[cfg(test)]
    pub async fn sample_count(&self) -> u64 {
        self.state.lock().await.sample_count
    }
}

async fn sample_generation(
    live_groups: Vec<phoenix_tools::bash::registry::LiveHandleProcessGroup>,
    terminal_sessions: BTreeSet<i32>,
) -> ResourceObservationGeneration {
    let api_identities = sysinfo::get_current_pid()
        .ok()
        .map(sysinfo::Pid::as_u32)
        .and_then(|pid| process_identity_for_sampling(pid).map(|identity| (pid, identity)))
        .into_iter()
        .collect::<BTreeMap<_, _>>();

    let mut handles = Vec::with_capacity(live_groups.len());
    let mut bash_identities = BTreeMap::<u32, ProcessIdentity>::new();
    let mut bash_attribution_available = true;
    for group in live_groups {
        let identities = group_member_identities_for_sampling(&BTreeSet::from([group.pgid]));
        match identities {
            Some(identities) if !identities.is_empty() => {
                let pids = identities.keys().copied().collect();
                bash_identities.extend(identities);
                handles.push(HandleObservationTarget {
                    scope_key: group.work_scope.stable_key(),
                    handle_id: group.handle_id.to_string(),
                    pids,
                });
            }
            Some(_) | None => bash_attribution_available = false,
        }
    }

    let terminal_identities = session_member_identities_for_sampling(&terminal_sessions);

    let mut all_identities = api_identities.clone();
    all_identities.extend(bash_identities);
    if let Some(identities) = &terminal_identities {
        all_identities.extend(identities.clone());
    }

    let (host, rows) = tokio::join!(
        super::deployment::sample_host_resources(),
        sample_process_observations(&all_identities)
    );

    ResourceObservationGeneration {
        sampled_at: Utc::now(),
        host,
        observations: rows.into_iter().map(|row| (row.pid, row)).collect(),
        api_pids: api_identities.keys().copied().collect(),
        terminal_pids: terminal_identities.map(|ids| ids.keys().copied().collect()),
        handles,
        bash_attribution_available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn empty_generation() -> ResourceObservationGeneration {
        ResourceObservationGeneration {
            sampled_at: Utc::now(),
            host: HostResources {
                logical_cpu_count: None,
                cpu_busy_percent: None,
                cpu_system_percent: None,
                cpu_idle_percent: None,
                total_memory_bytes: None,
                available_memory_bytes: None,
                used_memory_bytes: None,
                load_average_one: None,
                load_average_five: None,
                load_average_fifteen: None,
            },
            observations: BTreeMap::new(),
            api_pids: BTreeSet::new(),
            terminal_pids: Some(BTreeSet::new()),
            handles: Vec::new(),
            bash_attribution_available: true,
        }
    }

    #[tokio::test]
    async fn concurrent_consumers_share_one_generation() {
        let monitor = ResourceMonitor::new();
        let samples = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let monitor = monitor.clone();
            let samples = samples.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                monitor
                    .observe_with(
                        ObservationKey {
                            handles: Vec::new(),
                            terminal_sessions: Vec::new(),
                        },
                        || async move {
                            samples.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            empty_generation()
                        },
                    )
                    .await
            }));
        }
        let generations = futures::future::join_all(tasks).await;
        let first = generations[0].as_ref().unwrap();
        assert!(generations
            .iter()
            .all(|result| Arc::ptr_eq(first, result.as_ref().unwrap())));
        assert_eq!(samples.load(Ordering::SeqCst), 1);
        assert_eq!(monitor.sample_count().await, 1);
    }

    #[tokio::test]
    async fn ownership_change_invalidates_fresh_generation() {
        let monitor = ResourceMonitor::new();
        let samples = Arc::new(AtomicUsize::new(0));
        for key in [
            ObservationKey {
                handles: vec![("conversation:c1".into(), "b-1".into(), 10)],
                terminal_sessions: Vec::new(),
            },
            ObservationKey {
                handles: vec![("conversation:c1".into(), "b-2".into(), 20)],
                terminal_sessions: Vec::new(),
            },
        ] {
            let samples = samples.clone();
            monitor
                .observe_with(key, || async move {
                    samples.fetch_add(1, Ordering::SeqCst);
                    empty_generation()
                })
                .await;
        }
        assert_eq!(samples.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn deduplicates_scope_and_handle_membership() {
        let mut generation = empty_generation();
        generation.handles = vec![
            HandleObservationTarget {
                scope_key: "conversation:c1".into(),
                handle_id: "b-1".into(),
                pids: BTreeSet::from([10, 11]),
            },
            HandleObservationTarget {
                scope_key: "conversation:c1".into(),
                handle_id: "b-2".into(),
                pids: BTreeSet::from([11, 12]),
            },
        ];
        assert_eq!(
            generation.scope_pids("conversation:c1"),
            BTreeSet::from([10, 11, 12])
        );
        assert_eq!(generation.all_bash_pids(), BTreeSet::from([10, 11, 12]));
        assert_eq!(
            generation.handle_pids("conversation:c1", "b-1"),
            Some(&BTreeSet::from([10, 11]))
        );
    }
}
