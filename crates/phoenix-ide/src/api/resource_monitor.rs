//! Demand-driven, request-coalesced process resource observations.

use super::AppState;
use super::deployment::HostResources;
use super::process_sample::{
    ProcessIdentity, ProcessObservation, group_identities_by_pgid, process_identity_for_sampling,
    sample_process_observations, session_member_identities_for_sampling,
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const FRESHNESS_LEASE: Duration = Duration::from_millis(1_200);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservationKey {
    handles: Vec<(String, String, i32, String)>,
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
                    group.stable_resource_identity(),
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
    pub incomplete_bash_scopes: BTreeSet<String>,
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

    pub fn visible_handle_pids(&self, scope_key: &str, handle_ids: &[String]) -> BTreeSet<u32> {
        handle_ids
            .iter()
            .filter_map(|handle_id| self.handle_pids(scope_key, handle_id))
            .flat_map(|pids| pids.iter().copied())
            .collect()
    }

    pub fn bash_health_for_handles(
        &self,
        scope_key: &str,
        handle_ids: &[String],
    ) -> phoenix_core::domain::work_scope_inventory::ResourceHealth {
        let unavailable = || phoenix_core::domain::work_scope_inventory::ResourceHealth {
            cpu_percent: None,
            memory_bytes: None,
            process_count: None,
        };
        if self.incomplete_bash_scopes.contains(scope_key) || handle_ids.is_empty() {
            return unavailable();
        }
        let mut pids = BTreeSet::new();
        for handle_id in handle_ids {
            let Some(members) = self.handle_pids(scope_key, handle_id) else {
                return unavailable();
            };
            pids.extend(members);
        }
        if pids.is_empty()
            || pids.iter().any(|pid| !self.observations.contains_key(pid))
            || self
                .handles
                .iter()
                .any(|target| target.scope_key != scope_key && !target.pids.is_disjoint(&pids))
        {
            return unavailable();
        }
        health_for_pids(self, &pids)
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
        self.observe_sources(app.runtime.bash_handles(), &app.terminals)
            .await
    }

    pub async fn observe_sources(
        &self,
        handles: &phoenix_tools::bash::registry::BashHandleRegistry,
        terminals: &phoenix_terminal::ActiveTerminals,
    ) -> Arc<ResourceObservationGeneration> {
        let live_groups = handles.snapshot_live_process_groups().await;
        self.observe_live_groups(live_groups, terminals).await
    }

    pub async fn observe_live_groups(
        &self,
        live_groups: Vec<phoenix_tools::bash::registry::LiveHandleProcessGroup>,
        terminals: &phoenix_terminal::ActiveTerminals,
    ) -> Arc<ResourceObservationGeneration> {
        let terminal_sessions = terminals
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

fn valid_group_members(
    group: &phoenix_tools::bash::registry::LiveHandleProcessGroup,
    identities: &BTreeMap<u32, ProcessIdentity>,
) -> bool {
    identities
        .get(&group.launch_identity.process.pid)
        .is_some_and(|identity| identity.start_time == group.launch_identity.process.start_time)
}

type GroupIdentities = BTreeMap<i32, BTreeMap<u32, ProcessIdentity>>;

fn capture_bash_members(
    groups: &[phoenix_tools::bash::registry::LiveHandleProcessGroup],
    members: Option<&GroupIdentities>,
) -> (
    Vec<HandleObservationTarget>,
    BTreeMap<u32, ProcessIdentity>,
    BTreeSet<String>,
) {
    let mut handles = Vec::with_capacity(groups.len());
    let mut identities = BTreeMap::new();
    let mut incomplete = BTreeSet::new();
    for group in groups {
        let scope_key = group.work_scope.stable_key();
        match members.and_then(|groups| groups.get(&group.pgid)) {
            Some(group_members) if valid_group_members(group, group_members) => {
                handles.push(HandleObservationTarget {
                    scope_key,
                    handle_id: group.handle_id.to_string(),
                    pids: group_members.keys().copied().collect(),
                });
                identities.extend(
                    group_members
                        .iter()
                        .map(|(pid, identity)| (*pid, *identity)),
                );
            }
            _ => {
                incomplete.insert(scope_key);
            }
        }
    }
    (handles, identities, incomplete)
}

fn changed_bash_scopes(
    groups: &[phoenix_tools::bash::registry::LiveHandleProcessGroup],
    before: Option<&GroupIdentities>,
    after: Option<&GroupIdentities>,
) -> BTreeSet<String> {
    groups
        .iter()
        .filter(|group| {
            before.and_then(|members| members.get(&group.pgid))
                != after.and_then(|members| members.get(&group.pgid))
                || before.is_none()
                || after.is_none()
        })
        .map(|group| group.work_scope.stable_key())
        .collect()
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

    let pgids = live_groups.iter().map(|group| group.pgid).collect();
    let before_members = group_identities_by_pgid(&pgids);
    let (handles, bash_identities, mut incomplete_bash_scopes) =
        capture_bash_members(&live_groups, before_members.as_ref());

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

    let after_members = group_identities_by_pgid(&pgids);
    incomplete_bash_scopes.extend(changed_bash_scopes(
        &live_groups,
        before_members.as_ref(),
        after_members.as_ref(),
    ));

    ResourceObservationGeneration {
        sampled_at: Utc::now(),
        host,
        observations: rows.into_iter().map(|row| (row.pid, row)).collect(),
        api_pids: api_identities.keys().copied().collect(),
        terminal_pids: terminal_identities.map(|ids| ids.keys().copied().collect()),
        handles,
        incomplete_bash_scopes,
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
            incomplete_bash_scopes: BTreeSet::new(),
        }
    }

    #[test]
    fn post_sample_group_membership_changes_invalidate_only_affected_scopes() {
        use phoenix_core::process_identity::ProcessIdentity as BashProcessIdentity;
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use phoenix_tools::bash::handle::{BashLaunchIdentity, HandleId};

        let make_group = |scope: &str, pgid: i32| {
            let work_scope = ResourceScopeKey::Work(WorkScopeId::parse(scope).unwrap());
            phoenix_tools::bash::registry::LiveHandleProcessGroup {
                control_scope: work_scope.clone(),
                work_scope,
                handle_id: HandleId::new(format!("b-{scope}")),
                launch_identity: BashLaunchIdentity {
                    process: BashProcessIdentity {
                        pid: u32::try_from(pgid).unwrap(),
                        start_time: 1,
                    },
                    launch_uuid: scope.into(),
                },
                pgid,
            }
        };
        let groups = vec![make_group("a", 10), make_group("b", 20)];
        let identity = |pid, start_time| ProcessIdentity { pid, start_time };
        let before = BTreeMap::from([
            (
                10,
                BTreeMap::from([(10, identity(10, 1)), (11, identity(11, 2))]),
            ),
            (20, BTreeMap::from([(20, identity(20, 1))])),
        ]);
        let (targets, _, incomplete) = capture_bash_members(&groups, Some(&before));
        assert!(incomplete.is_empty());
        assert_eq!(targets[0].pids, BTreeSet::from([10, 11]));
        for changed in [
            BTreeMap::from([
                (10, identity(10, 1)),
                (11, identity(11, 2)),
                (12, identity(12, 3)),
            ]),
            BTreeMap::from([(10, identity(10, 1))]),
            BTreeMap::from([(10, identity(10, 1)), (11, identity(11, 3))]),
        ] {
            let after = BTreeMap::from([(10, changed), (20, before[&20].clone())]);
            assert_eq!(
                changed_bash_scopes(&groups, Some(&before), Some(&after)),
                BTreeSet::from(["work:a".to_string()])
            );
        }
        assert!(changed_bash_scopes(&groups, Some(&before), Some(&before)).is_empty());
        assert_eq!(
            changed_bash_scopes(&groups, Some(&before), None),
            BTreeSet::from(["work:a".to_string(), "work:b".to_string()])
        );
        let missing_leader = BTreeMap::from([(10, BTreeMap::from([(11, identity(11, 2))]))]);
        assert!(
            capture_bash_members(&groups, Some(&missing_leader))
                .2
                .contains("work:a")
        );
    }

    #[test]
    fn identity_read_failure_excludes_only_affected_group_from_cpu_and_memory() {
        use phoenix_core::process_identity::ProcessIdentity as BashProcessIdentity;
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use phoenix_tools::bash::handle::{BashLaunchIdentity, HandleId};

        let make_group = |name: &str, pgid: i32| {
            let scope = ResourceScopeKey::Work(WorkScopeId::parse(name).unwrap());
            phoenix_tools::bash::registry::LiveHandleProcessGroup {
                work_scope: scope.clone(),
                control_scope: scope,
                handle_id: HandleId::new(format!("b-{name}")),
                launch_identity: BashLaunchIdentity {
                    process: BashProcessIdentity {
                        pid: u32::try_from(pgid).unwrap(),
                        start_time: 1,
                    },
                    launch_uuid: name.into(),
                },
                pgid,
            }
        };
        let groups = vec![make_group("a", 10), make_group("b", 20)];
        let members = super::super::process_sample::collect_group_identities(
            &BTreeSet::from([10, 20]),
            [(10, 10), (11, 10), (12, 10), (20, 20)],
            |pid| Some(if pid == 20 { 20 } else { 10 }),
            |pid| (pid != 11).then_some(ProcessIdentity { pid, start_time: 1 }),
        );
        assert!(!members.contains_key(&10));
        assert_eq!(members[&20].len(), 1);
        let (handles, identities, incomplete) = capture_bash_members(&groups, Some(&members));
        assert_eq!(incomplete, BTreeSet::from(["work:a".into()]));
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].scope_key, "work:b");
        assert_eq!(identities.keys().copied().collect::<Vec<_>>(), vec![20]);
        let (_, complete) =
            super::super::work_scope_load::scope_cpu_members(&groups, Some(&members));
        assert!(!complete["work:a"]);
        assert!(complete["work:b"]);
    }

    #[test]
    fn moved_group_member_invalidates_source_and_destination_not_unrelated_group() {
        let members = super::super::process_sample::collect_group_identities(
            &BTreeSet::from([10, 20, 30]),
            [(10, 10), (11, 10), (20, 20), (30, 30)],
            |pid| {
                Some(if pid == 11 {
                    20
                } else {
                    i32::try_from(pid).unwrap()
                })
            },
            |pid| Some(ProcessIdentity { pid, start_time: 1 }),
        );
        assert!(!members.contains_key(&10));
        assert!(!members.contains_key(&20));
        assert_eq!(members[&30].keys().copied().collect::<Vec<_>>(), vec![30]);

        let unreadable = super::super::process_sample::collect_group_identities(
            &BTreeSet::from([10, 20]),
            [(10, 10), (11, 10), (20, 20)],
            |pid| (pid != 11).then_some(if pid == 20 { 20 } else { 10 }),
            |pid| Some(ProcessIdentity { pid, start_time: 1 }),
        );
        assert!(!unreadable.contains_key(&10));
        assert_eq!(unreadable[&20].len(), 1);
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
        assert!(
            generations
                .iter()
                .all(|result| Arc::ptr_eq(first, result.as_ref().unwrap()))
        );
        assert_eq!(samples.load(Ordering::SeqCst), 1);
        assert_eq!(monitor.sample_count().await, 1);
    }

    #[tokio::test]
    async fn ownership_change_invalidates_fresh_generation() {
        let monitor = ResourceMonitor::new();
        let samples = Arc::new(AtomicUsize::new(0));
        for key in [
            ObservationKey {
                handles: vec![(
                    "conversation:c1".into(),
                    "b-1".into(),
                    10,
                    "launch-a".into(),
                )],
                terminal_sessions: Vec::new(),
            },
            ObservationKey {
                handles: vec![(
                    "conversation:c1".into(),
                    "b-1".into(),
                    10,
                    "launch-b".into(),
                )],
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
    fn cache_key_tracks_launch_incarnation_without_pgid_or_handle_change() {
        use phoenix_core::process_identity::ProcessIdentity as BashProcessIdentity;
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use phoenix_tools::bash::handle::{BashLaunchIdentity, HandleId};
        let scope = ResourceScopeKey::Work(WorkScopeId::parse("a").unwrap());
        let mut group = phoenix_tools::bash::registry::LiveHandleProcessGroup {
            work_scope: scope.clone(),
            control_scope: scope,
            handle_id: HandleId::new("b-1"),
            launch_identity: BashLaunchIdentity {
                process: BashProcessIdentity {
                    pid: 10,
                    start_time: 1,
                },
                launch_uuid: "first".into(),
            },
            pgid: 10,
        };
        let before = ObservationKey::from_sources(&[group.clone()], &BTreeSet::new());
        group.launch_identity.launch_uuid = "second".into();
        let after = ObservationKey::from_sources(&[group.clone()], &BTreeSet::new());
        assert_ne!(before, after);
        let members = BTreeMap::from([(
            10,
            ProcessIdentity {
                pid: 10,
                start_time: 1,
            },
        )]);
        assert!(valid_group_members(&group, &members));
        group.launch_identity.process.start_time = 2;
        assert!(!valid_group_members(&group, &members));
    }

    #[test]
    fn missing_launch_leader_cannot_attribute_surviving_member_memory() {
        let group = phoenix_tools::bash::registry::LiveHandleProcessGroup {
            work_scope: phoenix_core::work_scope::ResourceScopeKey::Work(
                phoenix_core::work_scope::WorkScopeId::parse("a").unwrap(),
            ),
            control_scope: phoenix_core::work_scope::ResourceScopeKey::Work(
                phoenix_core::work_scope::WorkScopeId::parse("a").unwrap(),
            ),
            handle_id: phoenix_tools::bash::handle::HandleId::new("b-1"),
            launch_identity: phoenix_tools::bash::handle::BashLaunchIdentity {
                process: phoenix_core::process_identity::ProcessIdentity {
                    pid: 10,
                    start_time: 1,
                },
                launch_uuid: "launch".into(),
            },
            pgid: 10,
        };
        let members = BTreeMap::from([(
            11,
            ProcessIdentity {
                pid: 11,
                start_time: 2,
            },
        )]);
        assert!(!valid_group_members(&group, &members));
    }

    #[test]
    fn incomplete_scope_keeps_shared_group_claim_and_gates_other_scope_health() {
        use phoenix_core::process_identity::ProcessIdentity as BashProcessIdentity;
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use phoenix_tools::bash::handle::{BashLaunchIdentity, HandleId};
        use phoenix_tools::bash::registry::LiveHandleProcessGroup;

        let group = |scope: &str, handle: &str, pgid: i32| {
            let work_scope = ResourceScopeKey::Work(WorkScopeId::parse(scope).unwrap());
            LiveHandleProcessGroup {
                control_scope: work_scope.clone(),
                work_scope,
                handle_id: HandleId::new(handle),
                launch_identity: BashLaunchIdentity {
                    process: BashProcessIdentity {
                        pid: u32::try_from(pgid).unwrap(),
                        start_time: 1,
                    },
                    launch_uuid: handle.into(),
                },
                pgid,
            }
        };
        let groups = [
            group("a", "a-1", 10),
            group("b", "b-1", 10),
            group("a", "a-2", 30),
        ];
        let members = BTreeMap::from([
            (
                10,
                BTreeMap::from([(
                    10,
                    ProcessIdentity {
                        pid: 10,
                        start_time: 1,
                    },
                )]),
            ),
            (
                30,
                BTreeMap::from([(
                    30,
                    ProcessIdentity {
                        pid: 30,
                        start_time: 2,
                    },
                )]),
            ),
        ]);
        let (handles, identities, incomplete) = capture_bash_members(&groups, Some(&members));
        assert_eq!(incomplete, BTreeSet::from(["work:a".into()]));
        assert_eq!(
            identities.keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([10])
        );
        let mut generation = empty_generation();
        generation.handles = handles;
        generation.incomplete_bash_scopes = incomplete;
        generation.observations.insert(
            10,
            ProcessObservation {
                pid: 10,
                name: "shared".into(),
                cpu_percent: Some(1.0),
                memory_bytes: Some(1024),
                thread_count: None,
                cpu_time_seconds: None,
            },
        );
        assert_eq!(
            generation.handle_pids("work:a", "a-1"),
            Some(&BTreeSet::from([10]))
        );
        assert_eq!(generation.handle_pids("work:a", "a-2"), None);
        assert_eq!(generation.all_bash_pids(), BTreeSet::from([10]));
        for (scope, handle) in [("work:a", "a-1"), ("work:b", "b-1")] {
            let health = generation.bash_health_for_handles(scope, &[handle.into()]);
            assert_eq!(health.cpu_percent, None);
            assert_eq!(health.memory_bytes, None);
            assert_eq!(health.process_count, None);
        }
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
            generation.visible_handle_pids("conversation:c1", &["b-1".into()],),
            BTreeSet::from([10, 11])
        );
        assert_eq!(generation.all_bash_pids(), BTreeSet::from([10, 11, 12]));
        assert_eq!(
            generation.handle_pids("conversation:c1", "b-1"),
            Some(&BTreeSet::from([10, 11]))
        );
    }
}
