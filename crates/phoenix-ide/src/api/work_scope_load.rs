use super::process_sample::{cpu_time_for_identity, group_identities_by_pgid, ProcessIdentity};
use super::resource_monitor::{ResourceMonitor, ResourceObservationGeneration};
use chrono::{DateTime, Utc};
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_tools::bash::registry::{
    BashHandleRegistry, LiveHandleProcessGroup, LiveHandleSnapshot,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use ts_rs::TS;

const CADENCE: Duration = Duration::from_secs(5);
const HISTORY: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CpuLoadWindow {
    /// Average observed CPU cores, not exhaustive process-group accounting.
    pub average_cores: Option<f64>,
    /// Duration of valid sample intervals; does not imply complete CPU accounting.
    pub sampled_seconds: f64,
}

/// CPU and memory are attributed only to live tracked bash process groups.
/// Native sampling is approximate: children that finish between observations
/// may be missed, and tracking ends when the handle becomes terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WorkScopeLoadMeasurementBasis {
    LiveTrackedBashGroups,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct WorkScopeLoadSummary {
    pub scope_key: String,
    /// This summary observes surviving process identities in live tracked bash groups.
    pub measurement_basis: WorkScopeLoadMeasurementBasis,
    pub sampled_at: DateTime<Utc>,
    /// Zero means no tracked live jobs, not that the host is idle.
    pub live_jobs: u32,
    pub cpu_cores_latest: Option<f64>,
    pub cpu_sampled_at: Option<DateTime<Utc>>,
    pub cpu_one_minute: CpuLoadWindow,
    pub cpu_five_minutes: CpuLoadWindow,
    pub cpu_fifteen_minutes: CpuLoadWindow,
    pub memory_bytes: Option<u64>,
    pub process_count: Option<u32>,
}

#[derive(Clone)]
struct CpuReading {
    identity: ProcessIdentity,
    seconds: f64,
}

#[derive(Clone)]
struct CpuPoint {
    at: Instant,
    sampled_at: DateTime<Utc>,
    readings: BTreeMap<String, BTreeMap<u32, CpuReading>>,
    complete: BTreeMap<String, bool>,
    launches: BTreeMap<String, BTreeSet<String>>,
    lifecycle_epochs: BTreeMap<String, Option<u64>>,
}

struct CpuInterval {
    end: Instant,
    elapsed: Duration,
    cores: BTreeMap<String, Option<f64>>,
}

#[derive(Default)]
struct CpuState {
    previous: Option<CpuPoint>,
    intervals: VecDeque<CpuInterval>,
    latest: BTreeMap<String, Option<f64>>,
    #[cfg(test)]
    native_sample_count: usize,
}

pub struct WorkScopeLoadService {
    handles: Arc<BashHandleRegistry>,
    cpu: Mutex<CpuState>,
    memory_monitor: Arc<ResourceMonitor>,
    terminals: phoenix_terminal::ActiveTerminals,
    #[cfg(test)]
    before_cpu_lock: std::sync::Mutex<Option<Arc<tokio::sync::Barrier>>>,
}

impl WorkScopeLoadService {
    pub fn new(
        handles: &Arc<BashHandleRegistry>,
        memory_monitor: Arc<ResourceMonitor>,
        terminals: phoenix_terminal::ActiveTerminals,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            handles: handles.clone(),
            cpu: Mutex::new(CpuState::default()),
            memory_monitor,
            terminals,
            #[cfg(test)]
            before_cpu_lock: std::sync::Mutex::new(None),
        });
        Self::spawn_sampler(&service);
        service
    }

    pub fn memory_monitor(&self) -> Arc<ResourceMonitor> {
        self.memory_monitor.clone()
    }

    fn spawn_sampler(service: &Arc<Self>) {
        let weak = Arc::downgrade(service);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(CADENCE);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            timer.tick().await;
            loop {
                timer.tick().await;
                let Some(service) = weak.upgrade() else { break };
                service.tick().await;
            }
        });
    }

    async fn tick(&self) {
        let snapshot = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        let groups = &snapshot.groups;
        let mut state = self.cpu.lock().await;
        let now = Instant::now();
        prune_history(&mut state.intervals, now);
        if groups.is_empty() {
            state.previous = None;
            state.latest.clear();
            return;
        }
        let Ok((readings, complete, now, sampled_at)) = tokio::task::spawn_blocking({
            let groups = groups.clone();
            move || {
                let (readings, complete) = read_cpu(&groups);
                (readings, complete, Instant::now(), Utc::now())
            }
        })
        .await
        else {
            state.previous = None;
            state.latest.clear();
            return;
        };
        let epochs_after = self.handles.snapshot_lifecycle_epochs().await;
        let lifecycle_epochs = groups
            .iter()
            .map(|group| {
                let key = group.work_scope.stable_key();
                let before = snapshot
                    .lifecycle_epochs
                    .get(&group.work_scope)
                    .copied()
                    .flatten();
                let after = epochs_after.get(&group.work_scope).copied().flatten();
                (key, if before == after { before } else { None })
            })
            .collect();
        let launches = groups.iter().fold(
            BTreeMap::<String, BTreeSet<String>>::new(),
            |mut by_scope, group| {
                by_scope
                    .entry(group.work_scope.stable_key())
                    .or_default()
                    .insert(group.stable_resource_identity());
                by_scope
            },
        );
        let point = CpuPoint {
            at: now,
            sampled_at,
            readings,
            complete,
            launches,
            lifecycle_epochs,
        };
        if let Some(previous) = state.previous.take() {
            let elapsed = now.duration_since(previous.at);
            if elapsed <= CADENCE * 2 && !elapsed.is_zero() {
                let cores = cpu_deltas(&previous, &point, elapsed);
                state.latest = cores.clone();
                state.intervals.push_back(CpuInterval {
                    end: now,
                    elapsed,
                    cores,
                });
            } else {
                state.latest.clear();
            }
        }
        state.previous = Some(point);
        #[cfg(test)]
        {
            state.native_sample_count += 1;
        }
    }

    #[cfg(test)]
    async fn wait_before_cpu_lock(&self) {
        let barrier = self.before_cpu_lock.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            barrier.wait().await;
        }
    }

    pub async fn contains_scope(
        &self,
        db: &crate::db::Database,
        scope: &ResourceScopeKey,
    ) -> Result<bool, crate::db::DbError> {
        let ResourceScopeKey::Work(id) = scope else {
            return Ok(false);
        };
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM work_scopes WHERE id = ? AND lifecycle = 'active')",
        )
        .bind(id.as_str())
        .fetch_one(db.pool())
        .await?)
    }

    pub async fn summary(&self, scope: &ResourceScopeKey) -> WorkScopeLoadSummary {
        let snapshot = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        let groups = &snapshot.groups;
        let observation = if groups.iter().any(|group| group.work_scope == *scope) {
            Some(
                self.memory_monitor
                    .observe_live_groups(groups.clone(), &self.terminals)
                    .await,
            )
        } else {
            None
        };
        let latest = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        #[cfg(test)]
        self.wait_before_cpu_lock().await;
        let state = self.cpu.lock().await;
        let current = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        summarize(
            scope,
            &current.groups,
            stable_scope_epoch(&snapshot, &current, scope),
            observation.as_deref(),
            &state,
            Instant::now(),
            Utc::now(),
            memory_snapshot_stable(&snapshot, &latest, scope)
                && memory_snapshot_stable(&latest, &current, scope),
        )
    }

    pub async fn summaries(
        &self,
        db: &crate::db::Database,
    ) -> Result<Vec<WorkScopeLoadSummary>, crate::db::DbError> {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM work_scopes WHERE lifecycle = 'active' ORDER BY id")
                .fetch_all(db.pool())
                .await?;
        let scopes: Vec<_> = ids
            .into_iter()
            .filter_map(|id| ResourceScopeKey::from_stable_key(&format!("work:{id}")))
            .collect();
        let snapshot = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        let groups = &snapshot.groups;
        let observation = if groups
            .iter()
            .any(|group| scopes.contains(&group.work_scope))
        {
            Some(
                self.memory_monitor
                    .observe_live_groups(groups.clone(), &self.terminals)
                    .await,
            )
        } else {
            None
        };
        let latest = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        #[cfg(test)]
        self.wait_before_cpu_lock().await;
        let state = self.cpu.lock().await;
        let current = self
            .handles
            .snapshot_live_process_groups_with_epochs()
            .await;
        let now = Instant::now();
        let sampled_at = Utc::now();
        Ok(scopes
            .iter()
            .map(|scope| {
                summarize(
                    scope,
                    &current.groups,
                    stable_scope_epoch(&snapshot, &current, scope),
                    observation.as_deref(),
                    &state,
                    now,
                    sampled_at,
                    memory_snapshot_stable(&snapshot, &latest, scope)
                        && memory_snapshot_stable(&latest, &current, scope),
                )
            })
            .collect())
    }
}

fn stable_scope_epoch(
    before: &LiveHandleSnapshot,
    after: &LiveHandleSnapshot,
    scope: &ResourceScopeKey,
) -> Option<u64> {
    before
        .lifecycle_epochs
        .get(scope)
        .copied()
        .flatten()
        .filter(|epoch| after.lifecycle_epochs.get(scope) == Some(&Some(*epoch)))
}

fn memory_snapshot_stable(
    before: &LiveHandleSnapshot,
    after: &LiveHandleSnapshot,
    scope: &ResourceScopeKey,
) -> bool {
    let launches = |snapshot: &LiveHandleSnapshot| {
        snapshot
            .groups
            .iter()
            .filter(|group| group.work_scope == *scope)
            .map(|group| {
                (
                    group.handle_id.to_string(),
                    group.stable_resource_identity(),
                    group.pgid,
                )
            })
            .collect::<BTreeSet<_>>()
    };
    let before_launches = launches(before);
    before_launches == launches(after)
        && ![before, after].iter().any(|snapshot| {
            snapshot.groups.iter().any(|group| {
                group.work_scope == *scope
                    && snapshot
                        .groups
                        .iter()
                        .any(|other| other.work_scope != *scope && other.pgid == group.pgid)
            })
        })
        && (stable_scope_epoch(before, after, scope).is_some()
            || (before_launches.is_empty()
                && !before.lifecycle_epochs.contains_key(scope)
                && !after.lifecycle_epochs.contains_key(scope)))
}

fn prune_history(history: &mut VecDeque<CpuInterval>, now: Instant) {
    while history
        .front()
        .is_some_and(|interval| now.duration_since(interval.end) >= HISTORY)
    {
        history.pop_front();
    }
}

#[allow(clippy::too_many_arguments)]
fn summarize(
    scope: &ResourceScopeKey,
    groups: &[LiveHandleProcessGroup],
    lifecycle_epoch: Option<u64>,
    observation: Option<&ResourceObservationGeneration>,
    state: &CpuState,
    now: Instant,
    sampled_at: DateTime<Utc>,
    snapshot_stable: bool,
) -> WorkScopeLoadSummary {
    let scoped: Vec<_> = groups
        .iter()
        .filter(|group| group.work_scope == *scope)
        .collect();
    let key = scope.stable_key();
    let active = !scoped.is_empty();
    let launches: BTreeSet<_> = scoped
        .iter()
        .map(|group| group.stable_resource_identity())
        .collect();
    let recent = state.previous.as_ref().is_some_and(|point| {
        now.duration_since(point.at) <= CADENCE * 2
            && point.launches.get(&key) == Some(&launches)
            && lifecycle_epoch.is_some()
            && point.lifecycle_epochs.get(&key) == Some(&lifecycle_epoch)
    });
    let latest = if !snapshot_stable {
        None
    } else if active {
        recent
            .then(|| state.latest.get(&key).copied().flatten())
            .flatten()
    } else {
        Some(0.0)
    };
    let (memory_bytes, process_count) = if !snapshot_stable {
        (None, None)
    } else if !active {
        (Some(0), Some(0))
    } else if let Some(observation) = observation {
        let ids = scoped
            .iter()
            .map(|group| group.handle_id.to_string())
            .collect::<Vec<_>>();
        let pids = observation.visible_handle_pids(&key, &ids);
        let conflicting = observation
            .handles
            .iter()
            .any(|target| target.scope_key != key && !target.pids.is_disjoint(&pids));
        if conflicting
            || pids.is_empty()
            || observation.incomplete_bash_scopes.contains(&key)
            || pids
                .iter()
                .any(|pid| !observation.observations.contains_key(pid))
        {
            (None, None)
        } else {
            let health = super::resource_monitor::health_for_pids(observation, &pids);
            (health.memory_bytes, health.process_count)
        }
    } else {
        (None, None)
    };
    WorkScopeLoadSummary {
        scope_key: key.clone(),
        measurement_basis: WorkScopeLoadMeasurementBasis::LiveTrackedBashGroups,
        sampled_at: observation.map_or(sampled_at, |value| value.sampled_at),
        live_jobs: scoped.len().try_into().unwrap_or(u32::MAX),
        cpu_cores_latest: latest,
        cpu_sampled_at: (active && recent && latest.is_some())
            .then(|| state.previous.as_ref().map(|point| point.sampled_at))
            .flatten(),
        cpu_one_minute: window(&state.intervals, &key, now, 60),
        cpu_five_minutes: window(&state.intervals, &key, now, 300),
        cpu_fifteen_minutes: window(&state.intervals, &key, now, 900),
        memory_bytes,
        process_count,
    }
}

fn read_cpu(
    groups: &[LiveHandleProcessGroup],
) -> (
    BTreeMap<String, BTreeMap<u32, CpuReading>>,
    BTreeMap<String, bool>,
) {
    read_cpu_with(groups, group_identities_by_pgid, cpu_time_for_identity)
}

fn read_cpu_with(
    groups: &[LiveHandleProcessGroup],
    mut enumerate: impl FnMut(&BTreeSet<i32>) -> Option<BTreeMap<i32, BTreeMap<u32, ProcessIdentity>>>,
    mut counter: impl FnMut(ProcessIdentity) -> Option<f64>,
) -> (
    BTreeMap<String, BTreeMap<u32, CpuReading>>,
    BTreeMap<String, bool>,
) {
    let pgids = groups
        .iter()
        .map(|group| group.pgid)
        .collect::<BTreeSet<_>>();
    let before = enumerate(&pgids);
    let (members, mut complete) = scope_cpu_members(groups, before.as_ref());
    let mut readings = BTreeMap::new();
    for (scope, identities) in &members {
        let mut scope_readings = BTreeMap::new();
        if complete.get(scope) == Some(&true) {
            for (&pid, &identity) in identities {
                if let Some(seconds) = counter(identity) {
                    scope_readings.insert(pid, CpuReading { identity, seconds });
                } else {
                    complete.insert(scope.clone(), false);
                }
            }
        }
        readings.insert(scope.clone(), scope_readings);
    }
    let after = enumerate(&pgids);
    let (after_members, after_complete) = scope_cpu_members(groups, after.as_ref());
    for (scope, valid) in &mut complete {
        *valid &= after_complete.get(scope) == Some(&true)
            && members.get(scope) == after_members.get(scope);
    }
    for (scope, valid) in &complete {
        if !valid {
            tracing::debug!(
                scope,
                "WorkScope CPU observation incomplete; excluding interval coverage"
            );
        }
    }
    (readings, complete)
}

pub(super) fn scope_cpu_members(
    groups: &[LiveHandleProcessGroup],
    group_members: Option<&BTreeMap<i32, BTreeMap<u32, ProcessIdentity>>>,
) -> (
    BTreeMap<String, BTreeMap<u32, ProcessIdentity>>,
    BTreeMap<String, bool>,
) {
    let mut members_by_scope = BTreeMap::<String, BTreeMap<u32, ProcessIdentity>>::new();
    let mut complete = BTreeMap::<String, bool>::new();
    let mut owners = BTreeMap::<(u32, u128), BTreeSet<String>>::new();
    let conflicts = super::resource_monitor::conflicting_bash_scopes(groups);
    for group in groups {
        let scope = group.work_scope.stable_key();
        let valid = complete
            .entry(scope.clone())
            .or_insert(!conflicts.contains(&scope));
        let Some(members) = group_members.and_then(|groups| groups.get(&group.pgid)) else {
            *valid = false;
            members_by_scope.entry(scope).or_default();
            continue;
        };
        if members
            .get(&group.launch_identity.process.pid)
            .is_none_or(|identity| identity.start_time != group.launch_identity.process.start_time)
        {
            *valid = false;
            members_by_scope.entry(scope).or_default();
            continue;
        }
        let scope_members = members_by_scope.entry(scope.clone()).or_default();
        for (&pid, &identity) in members {
            if scope_members
                .insert(pid, identity)
                .is_some_and(|old| old != identity)
            {
                *valid = false;
            }
            owners
                .entry((identity.pid, identity.start_time))
                .or_default()
                .insert(scope.clone());
        }
    }
    for scopes in owners.values().filter(|scopes| scopes.len() > 1) {
        for scope in scopes {
            complete.insert(scope.clone(), false);
        }
    }
    (members_by_scope, complete)
}

fn cpu_deltas(
    before: &CpuPoint,
    after: &CpuPoint,
    elapsed: Duration,
) -> BTreeMap<String, Option<f64>> {
    let mut result = BTreeMap::new();
    for (scope, current) in &after.readings {
        let mut seconds = 0.0;
        let mut valid = after.complete.get(scope) == Some(&true)
            && before.complete.get(scope) == Some(&true)
            && after.launches.get(scope) == before.launches.get(scope)
            && after.lifecycle_epochs.get(scope).is_some_and(|epoch| {
                epoch.is_some() && before.lifecycle_epochs.get(scope) == Some(epoch)
            })
            && before
                .readings
                .get(scope)
                .is_some_and(|old| old.len() == current.len());
        for (pid, reading) in current {
            match before.readings.get(scope).and_then(|rows| rows.get(pid)) {
                Some(prior)
                    if prior.identity == reading.identity && reading.seconds >= prior.seconds =>
                {
                    seconds += reading.seconds - prior.seconds;
                }
                _ => valid = false,
            }
        }
        result.insert(
            scope.clone(),
            valid.then_some(seconds / elapsed.as_secs_f64()),
        );
    }
    result
}

fn window(
    history: &VecDeque<CpuInterval>,
    scope: &str,
    now: Instant,
    seconds: u64,
) -> CpuLoadWindow {
    let start = now
        .checked_sub(Duration::from_secs(seconds))
        .unwrap_or_else(|| {
            history
                .front()
                .and_then(|interval| interval.end.checked_sub(interval.elapsed))
                .unwrap_or(now)
        });
    let mut weighted = 0.0;
    let mut coverage = 0.0;
    for interval in history {
        if let Some(Some(cores)) = interval.cores.get(scope) {
            let Some(interval_start) = interval.end.checked_sub(interval.elapsed) else {
                continue;
            };
            let overlap_start = interval_start.max(start);
            let overlap_end = interval.end.min(now);
            if overlap_end > overlap_start {
                let duration = (overlap_end - overlap_start).as_secs_f64();
                weighted += duration * cores;
                coverage += duration;
            }
        }
    }
    CpuLoadWindow {
        average_cores: (coverage > 0.0).then_some(weighted / coverage),
        sampled_seconds: coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_snapshot_rejects_after_start_replacement_and_exit() {
        use std::collections::HashMap;
        let a = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let b = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("b").unwrap());
        let before = LiveHandleSnapshot {
            groups: vec![group(a.clone(), "b-1")],
            lifecycle_epochs: HashMap::from([(a.clone(), Some(1)), (b.clone(), Some(0))]),
        };
        let mut after = before.clone();
        assert!(memory_snapshot_stable(&before, &after, &a));
        assert!(memory_snapshot_stable(&before, &after, &b));
        after.groups.push(group(a.clone(), "b-2"));
        after.lifecycle_epochs.insert(a.clone(), Some(2));
        assert!(!memory_snapshot_stable(&before, &after, &a));
        assert!(memory_snapshot_stable(&before, &after, &b));
        let summary = summarize(
            &a,
            &before.groups,
            None,
            None,
            &CpuState::default(),
            Instant::now(),
            Utc::now(),
            memory_snapshot_stable(&before, &after, &a),
        );
        assert_eq!((summary.memory_bytes, summary.process_count), (None, None));
        after = before.clone();
        after.groups[0].launch_identity.launch_uuid = "replacement".into();
        assert!(!memory_snapshot_stable(&before, &after, &a));
        after = before.clone();
        after.groups.clear();
        assert!(!memory_snapshot_stable(&before, &after, &a));
        let idle = LiveHandleSnapshot {
            groups: Vec::new(),
            lifecycle_epochs: HashMap::from([(a.clone(), Some(1))]),
        };
        assert!(memory_snapshot_stable(&idle, &idle, &a));
        assert_eq!(
            summarize(
                &a,
                &[],
                None,
                None,
                &CpuState::default(),
                Instant::now(),
                Utc::now(),
                memory_snapshot_stable(&idle, &idle, &a)
            )
            .memory_bytes,
            Some(0)
        );
        assert!(!memory_snapshot_stable(&idle, &before, &a));
        let started = summarize(
            &a,
            &[],
            None,
            None,
            &CpuState::default(),
            Instant::now(),
            Utc::now(),
            memory_snapshot_stable(&idle, &before, &a),
        );
        assert_eq!((started.memory_bytes, started.process_count), (None, None));
        let missing = LiveHandleSnapshot {
            groups: vec![],
            lifecycle_epochs: HashMap::new(),
        };
        assert!(!memory_snapshot_stable(&missing, &before, &a));
    }

    #[test]
    fn idle_to_live_race_reports_post_snapshot_liveness_without_idle_cpu() {
        use std::collections::HashMap;
        let a = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let before = LiveHandleSnapshot {
            groups: vec![],
            lifecycle_epochs: HashMap::from([(a.clone(), Some(0))]),
        };
        let after = LiveHandleSnapshot {
            groups: vec![group(a.clone(), "b-1")],
            lifecycle_epochs: HashMap::from([(a.clone(), Some(1))]),
        };
        let stable = memory_snapshot_stable(&before, &after, &a);
        assert!(!stable);
        let summary = summarize(
            &a,
            &after.groups,
            stable_scope_epoch(&before, &after, &a),
            None,
            &CpuState::default(),
            Instant::now(),
            Utc::now(),
            stable,
        );
        assert_eq!(summary.live_jobs, 1);
        assert_eq!(summary.cpu_cores_latest, None);
        assert_eq!(summary.cpu_sampled_at, None);
        assert_eq!((summary.memory_bytes, summary.process_count), (None, None));
    }

    #[test]
    fn new_other_scope_pgid_claim_invalidates_both_even_with_unchanged_owner_epoch() {
        use std::collections::HashMap;
        let a = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let b = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("b").unwrap());
        let before = LiveHandleSnapshot {
            groups: vec![group(a.clone(), "b-1")],
            lifecycle_epochs: HashMap::from([(a.clone(), Some(1)), (b.clone(), Some(0))]),
        };
        let mut after = before.clone();
        let mut other = group(b.clone(), "b-2");
        other.launch_identity.launch_uuid = "different-launch".into();
        after.groups.push(other);
        after.lifecycle_epochs.insert(b.clone(), Some(1));
        assert_eq!(stable_scope_epoch(&before, &after, &a), Some(1));
        assert!(!memory_snapshot_stable(&before, &after, &a));
        assert!(!memory_snapshot_stable(&before, &after, &b));
        assert!(!memory_snapshot_stable(&after, &after, &a));
        assert!(!memory_snapshot_stable(&after, &after, &b));

        let now = Instant::now();
        let mut state = CpuState {
            previous: Some(CpuPoint {
                at: now,
                sampled_at: Utc::now(),
                readings: BTreeMap::new(),
                complete: BTreeMap::new(),
                launches: BTreeMap::from([(
                    a.stable_key(),
                    BTreeSet::from([before.groups[0].stable_resource_identity()]),
                )]),
                lifecycle_epochs: BTreeMap::from([(a.stable_key(), Some(1))]),
            }),
            ..CpuState::default()
        };
        state.latest.insert(a.stable_key(), Some(0.7));
        for scope in [&a, &b] {
            let summary = summarize(
                scope,
                &after.groups,
                stable_scope_epoch(&before, &after, scope),
                None,
                &state,
                now,
                Utc::now(),
                memory_snapshot_stable(&before, &after, scope),
            );
            assert_eq!(summary.live_jobs, 1);
            assert_eq!(summary.cpu_cores_latest, None);
            assert_eq!(summary.cpu_sampled_at, None);
            assert_eq!((summary.memory_bytes, summary.process_count), (None, None));
        }
    }

    #[tokio::test]
    async fn summary_rechecks_registry_after_waiting_for_cpu_mutex() {
        use phoenix_tools::bash::handle::{Handle, HandleId};
        let handles = Arc::new(BashHandleRegistry::new());
        let load = WorkScopeLoadService::new(
            &handles,
            ResourceMonitor::new(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        let scope = ResourceScopeKey::Work(
            phoenix_core::work_scope::WorkScopeId::parse("waiting").unwrap(),
        );
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        *load.before_cpu_lock.lock().unwrap() = Some(barrier.clone());
        let cpu = load.cpu.lock().await;
        let pending = tokio::spawn({
            let load = load.clone();
            let scope = scope.clone();
            async move { load.summary(&scope).await }
        });
        barrier.wait().await;
        handles
            .register_existing_handle(
                &scope,
                Handle::new_live(
                    scope.clone(),
                    HandleId::new("new"),
                    "test".into(),
                    None,
                    "/tmp".into(),
                    i32::MAX,
                    10,
                    1024,
                ),
            )
            .await;
        drop(cpu);
        let result = pending.await.unwrap();
        assert_eq!(result.live_jobs, 1);
        assert_eq!(result.cpu_cores_latest, None);
        assert_eq!((result.memory_bytes, result.process_count), (None, None));
    }

    #[tokio::test]
    async fn summaries_recheck_registry_after_waiting_for_cpu_mutex() {
        use phoenix_tools::bash::handle::{Handle, HandleId};
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("waiting-conv", "waiting", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope = ResourceScopeKey::Work(
            db.get_conversation("waiting-conv")
                .await
                .unwrap()
                .attached_work_scope_id
                .unwrap(),
        );
        let handles = Arc::new(BashHandleRegistry::new());
        let load = WorkScopeLoadService::new(
            &handles,
            ResourceMonitor::new(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        *load.before_cpu_lock.lock().unwrap() = Some(barrier.clone());
        let cpu = load.cpu.lock().await;
        let pending = tokio::spawn({
            let load = load.clone();
            async move { load.summaries(&db).await.unwrap() }
        });
        barrier.wait().await;
        handles
            .register_existing_handle(
                &scope,
                Handle::new_live(
                    scope.clone(),
                    HandleId::new("new"),
                    "test".into(),
                    None,
                    "/tmp".into(),
                    i32::MAX,
                    10,
                    1024,
                ),
            )
            .await;
        drop(cpu);
        let results = pending.await.unwrap();
        let result = results
            .iter()
            .find(|item| item.scope_key == scope.stable_key())
            .unwrap();
        assert_eq!(result.live_jobs, 1);
        assert_eq!(result.cpu_cores_latest, None);
        assert_eq!((result.memory_bytes, result.process_count), (None, None));
    }

    #[tokio::test]
    async fn empty_scope_does_not_invoke_native_or_demand_monitor() {
        let handles = Arc::new(BashHandleRegistry::new());
        let monitor = ResourceMonitor::new();
        let load = WorkScopeLoadService::new(
            &handles,
            monitor.clone(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        load.tick().await;
        let summary = load
            .summary(&ResourceScopeKey::Work(
                phoenix_core::work_scope::WorkScopeId::parse("empty").unwrap(),
            ))
            .await;
        assert_eq!(summary.live_jobs, 0);
        assert_eq!(summary.cpu_cores_latest, Some(0.0));
        assert_eq!(summary.memory_bytes, Some(0));
        assert_eq!(load.cpu.lock().await.native_sample_count, 0);
        assert_eq!(monitor.sample_count().await, 0);
    }

    #[tokio::test]
    async fn idle_tick_expires_retained_history_without_native_sampling() {
        let handles = Arc::new(BashHandleRegistry::new());
        let monitor = ResourceMonitor::new();
        let load = WorkScopeLoadService::new(
            &handles,
            monitor.clone(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        {
            let mut state = load.cpu.lock().await;
            state.intervals.push_back(CpuInterval {
                end: Instant::now().checked_sub(HISTORY).unwrap(),
                elapsed: CADENCE,
                cores: BTreeMap::from([("work:a".into(), Some(1.0))]),
            });
        }
        load.tick().await;
        let state = load.cpu.lock().await;
        assert!(state.intervals.is_empty());
        assert_eq!(state.native_sample_count, 0);
        assert_eq!(monitor.sample_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_scope_requests_share_a_demand_observation() {
        use phoenix_tools::bash::handle::{Handle, HandleId};
        let handles = Arc::new(BashHandleRegistry::new());
        let monitor = ResourceMonitor::new();
        let load = WorkScopeLoadService::new(
            &handles,
            monitor.clone(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("active").unwrap());
        handles
            .register_existing_handle(
                &scope,
                Handle::new_live(
                    scope.clone(),
                    HandleId::new("b-1"),
                    "test".into(),
                    None,
                    "/tmp".into(),
                    i32::MAX,
                    10,
                    1024,
                ),
            )
            .await;
        let (a, b) = tokio::join!(load.summary(&scope), load.summary(&scope));
        assert_eq!(a.live_jobs, 1);
        assert_eq!(b.live_jobs, 1);
        assert_eq!(a.sampled_at, b.sampled_at);
        assert_eq!(monitor.sample_count().await, 1);
        assert_eq!(load.cpu.lock().await.native_sample_count, 0);
    }

    #[tokio::test]
    async fn all_summaries_share_one_observation_for_multiple_live_scopes() {
        use phoenix_tools::bash::handle::{Handle, HandleId};
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let handles = Arc::new(BashHandleRegistry::new());
        let monitor = ResourceMonitor::new();
        let load = WorkScopeLoadService::new(
            &handles,
            monitor.clone(),
            phoenix_terminal::ActiveTerminals::new(),
        );
        for id in ["all-load-a", "all-load-b"] {
            db.create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
            let conversation = db.get_conversation(id).await.unwrap();
            let scope = ResourceScopeKey::Work(conversation.attached_work_scope_id.unwrap());
            handles
                .register_existing_handle(
                    &scope,
                    Handle::new_live(
                        scope.clone(),
                        HandleId::new(id),
                        "test".into(),
                        None,
                        "/tmp".into(),
                        i32::MAX,
                        10,
                        1024,
                    ),
                )
                .await;
        }
        let summaries = load.summaries(&db).await.unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|item| item.live_jobs == 1));
        assert_eq!(summaries[0].sampled_at, summaries[1].sampled_at);
        assert_eq!(monitor.sample_count().await, 1);
    }

    #[test]
    fn gaps_and_partial_windows_do_not_fabricate_coverage() {
        let now = Instant::now();
        let mut history = VecDeque::new();
        history.push_back(CpuInterval {
            end: now.checked_sub(Duration::from_secs(30)).unwrap(),
            elapsed: Duration::from_secs(10),
            cores: BTreeMap::from([("work:a".into(), Some(2.0))]),
        });
        history.push_back(CpuInterval {
            end: now,
            elapsed: Duration::from_secs(5),
            cores: BTreeMap::from([("work:a".into(), None)]),
        });
        let result = window(&history, "work:a", now, 60);
        assert_eq!(result.average_cores, Some(2.0));
        assert!((result.sampled_seconds - 10.0).abs() < f64::EPSILON);
        assert_eq!(window(&history, "work:b", now, 60).average_cores, None);
    }
    fn point(now: Instant, seconds: f64) -> CpuPoint {
        CpuPoint {
            at: now,
            sampled_at: Utc::now(),
            readings: BTreeMap::from([(
                "work:a".into(),
                BTreeMap::from([
                    (
                        10,
                        CpuReading {
                            identity: ProcessIdentity {
                                pid: 10,
                                start_time: 1,
                            },
                            seconds,
                        },
                    ),
                    (
                        11,
                        CpuReading {
                            identity: ProcessIdentity {
                                pid: 11,
                                start_time: 2,
                            },
                            seconds: seconds / 2.0,
                        },
                    ),
                ]),
            )]),
            complete: BTreeMap::from([("work:a".into(), true)]),
            launches: BTreeMap::from([("work:a".into(), BTreeSet::from(["launch".into()]))]),
            lifecycle_epochs: BTreeMap::from([("work:a".into(), Some(1))]),
        }
    }

    #[test]
    fn cpu_deltas_require_complete_stable_membership_and_launch() {
        let before = point(Instant::now(), 10.0);
        let mut after = point(before.at + Duration::from_secs(5), 15.0);
        assert_eq!(
            cpu_deltas(&before, &after, Duration::from_secs(5))["work:a"],
            Some(1.5)
        );
        after.readings.get_mut("work:a").unwrap().remove(&11);
        assert_eq!(
            cpu_deltas(&before, &after, Duration::from_secs(5))["work:a"],
            None
        );
        after = point(before.at, 15.0);
        after
            .readings
            .get_mut("work:a")
            .unwrap()
            .get_mut(&10)
            .unwrap()
            .identity
            .start_time = 9;
        assert_eq!(
            cpu_deltas(&before, &after, Duration::from_secs(5))["work:a"],
            None
        );
        after = point(before.at, 15.0);
        after
            .launches
            .get_mut("work:a")
            .unwrap()
            .insert("other".into());
        assert_eq!(
            cpu_deltas(&before, &after, Duration::from_secs(5))["work:a"],
            None
        );
        after = point(before.at, 15.0);
        after.complete.insert("work:a".into(), false);
        assert_eq!(
            cpu_deltas(&before, &after, Duration::from_secs(5))["work:a"],
            None
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn transient_handle_between_samples_invalidates_only_crossed_interval() {
        use phoenix_tools::bash::handle::{FinalCause, Handle, HandleId, TOMBSTONE_TAIL_LINES};
        let registry = BashHandleRegistry::new();
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let new_handle = |id: &str, pid: u32| {
            Handle::new_live(
                scope.clone(),
                HandleId::new(id),
                id.into(),
                None,
                "/tmp".into(),
                i32::try_from(pid).unwrap(),
                pid,
                1024,
            )
        };
        registry
            .register_existing_handle(&scope, new_handle("long", 10))
            .await;
        let before_snapshot = registry.snapshot_live_process_groups_with_epochs().await;
        let before_epoch = before_snapshot.lifecycle_epochs[&scope];
        assert_eq!(before_snapshot.groups.len(), 1);
        let transient = new_handle("short", 11);
        registry
            .register_existing_handle(&scope, transient.clone())
            .await;
        assert!(
            transient
                .transition_to_terminal(
                    FinalCause::Exited { exit_code: Some(0) },
                    Duration::from_secs(1),
                    std::time::SystemTime::now(),
                    TOMBSTONE_TAIL_LINES
                )
                .await
        );
        let after_snapshot = registry.snapshot_live_process_groups_with_epochs().await;
        let after_epoch = after_snapshot.lifecycle_epochs[&scope];
        assert_eq!(after_snapshot.groups, before_snapshot.groups);
        assert_eq!(after_epoch.unwrap(), before_epoch.unwrap() + 2);

        let now = Instant::now();
        let mut before = point(now, 10.0);
        before
            .lifecycle_epochs
            .insert(scope.stable_key(), before_epoch);
        let mut after = point(now + CADENCE, 15.0);
        after
            .lifecycle_epochs
            .insert(scope.stable_key(), after_epoch);
        let live_launches = BTreeSet::from([after_snapshot.groups[0].stable_resource_identity()]);
        before
            .launches
            .insert(scope.stable_key(), live_launches.clone());
        after
            .launches
            .insert(scope.stable_key(), live_launches.clone());
        assert_eq!(cpu_deltas(&before, &after, CADENCE)["work:a"], None);
        let mut state = CpuState {
            previous: Some(after.clone()),
            latest: cpu_deltas(&before, &after, CADENCE),
            ..CpuState::default()
        };
        state.intervals.push_back(CpuInterval {
            end: after.at,
            elapsed: CADENCE,
            cores: state.latest.clone(),
        });
        let summarize_live = |state: &CpuState, at| {
            summarize(
                &scope,
                &after_snapshot.groups,
                after_epoch,
                None,
                state,
                at,
                Utc::now(),
                true,
            )
        };
        let summary = summarize_live(&state, after.at);
        assert_eq!(summary.cpu_cores_latest, None);
        assert_eq!(summary.cpu_sampled_at, None);
        assert!(summary.cpu_one_minute.sampled_seconds.abs() < f64::EPSILON);

        let mut stable = point(after.at + CADENCE, 20.0);
        stable
            .lifecycle_epochs
            .insert(scope.stable_key(), after_epoch);
        stable.launches.insert(scope.stable_key(), live_launches);
        assert_eq!(cpu_deltas(&after, &stable, CADENCE)["work:a"], Some(1.5));
        state.latest = cpu_deltas(&after, &stable, CADENCE);
        state.previous = Some(stable.clone());
        state.intervals.push_back(CpuInterval {
            end: stable.at,
            elapsed: CADENCE,
            cores: state.latest.clone(),
        });
        let summary = summarize_live(&state, stable.at);
        assert_eq!(summary.cpu_cores_latest, Some(1.5));
        assert!(
            (summary.cpu_one_minute.sampled_seconds - CADENCE.as_secs_f64()).abs() < f64::EPSILON
        );
    }

    #[test]
    fn unavailable_latest_has_no_cpu_timestamp() {
        let now = Instant::now();
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let groups = vec![group(scope.clone(), "b-1")];
        let before = point(now.checked_sub(CADENCE).unwrap(), 2.0);
        let mut after = point(now, 3.0);
        for invalid in ["membership", "counter"] {
            after.readings = point(now, 3.0).readings;
            if invalid == "membership" {
                after.readings.get_mut("work:a").unwrap().remove(&11);
            } else {
                after
                    .readings
                    .get_mut("work:a")
                    .unwrap()
                    .get_mut(&10)
                    .unwrap()
                    .seconds = 1.0;
            }
            let mut state = CpuState {
                previous: Some(after.clone()),
                latest: cpu_deltas(&before, &after, CADENCE),
                ..CpuState::default()
            };
            state.previous.as_mut().unwrap().launches.insert(
                scope.stable_key(),
                BTreeSet::from([groups[0].stable_resource_identity()]),
            );
            assert!(state.latest.contains_key(&scope.stable_key()));
            let summary = summarize(
                &scope,
                &groups,
                Some(1),
                None,
                &state,
                now,
                Utc::now(),
                true,
            );
            assert_eq!(summary.cpu_cores_latest, None, "{invalid}");
            assert_eq!(summary.cpu_sampled_at, None, "{invalid}");
            state.latest.insert(scope.stable_key(), Some(0.0));
            let summary = summarize(
                &scope,
                &groups,
                Some(2),
                None,
                &state,
                now,
                Utc::now(),
                true,
            );
            assert_eq!(summary.cpu_cores_latest, None);
            assert_eq!(summary.cpu_sampled_at, None);
        }
    }

    #[test]
    fn windows_clip_boundary_and_idle_prunes_history() {
        let now = Instant::now();
        let mut history = VecDeque::from([
            CpuInterval {
                end: now.checked_sub(Duration::from_secs(55)).unwrap(),
                elapsed: Duration::from_secs(10),
                cores: BTreeMap::from([("work:a".into(), Some(2.0))]),
            },
            CpuInterval {
                end: now.checked_sub(Duration::from_secs(20)).unwrap(),
                elapsed: Duration::from_secs(10),
                cores: BTreeMap::from([("work:a".into(), Some(1.0))]),
            },
        ]);
        let result = window(&history, "work:a", now, 60);
        assert!((result.sampled_seconds - 15.0).abs() < f64::EPSILON);
        assert_eq!(result.average_cores, Some(20.0 / 15.0));
        prune_history(&mut history, now + HISTORY);
        assert!(history.is_empty());
    }

    fn group(scope: ResourceScopeKey, id: &str) -> LiveHandleProcessGroup {
        use phoenix_core::process_identity::ProcessIdentity as BashProcessIdentity;
        use phoenix_tools::bash::handle::{BashLaunchIdentity, HandleId};
        LiveHandleProcessGroup {
            work_scope: scope.clone(),
            control_scope: scope,
            handle_id: HandleId::new(id),
            launch_identity: BashLaunchIdentity {
                process: BashProcessIdentity {
                    pid: 10,
                    start_time: 1,
                },
                launch_uuid: "launch".into(),
            },
            pgid: 10,
        }
    }

    #[test]
    fn cpu_counter_interval_rechecks_native_membership_and_identity() {
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let groups = vec![group(scope, "b-1")];
        let leader = ProcessIdentity {
            pid: 10,
            start_time: 1,
        };
        let child = ProcessIdentity {
            pid: 11,
            start_time: 2,
        };
        let members = |entries: &[ProcessIdentity]| {
            Some(BTreeMap::from([(
                10,
                entries
                    .iter()
                    .map(|identity| (identity.pid, *identity))
                    .collect(),
            )]))
        };
        let base = members(&[leader, child]);
        let cases = [
            ("stable", base.clone(), base.clone(), true),
            ("join", members(&[leader]), base.clone(), false),
            ("exit", base.clone(), members(&[leader]), false),
            (
                "reuse",
                base.clone(),
                members(&[
                    leader,
                    ProcessIdentity {
                        pid: 11,
                        start_time: 3,
                    },
                ]),
                false,
            ),
            ("failed post-read", base.clone(), None, false),
        ];
        for (case, before, after, expected) in cases {
            let mut calls = 0;
            let (readings, complete) = read_cpu_with(
                &groups,
                |_| {
                    calls += 1;
                    if calls == 1 {
                        before.clone()
                    } else {
                        after.clone()
                    }
                },
                |_| Some(2.0),
            );
            assert_eq!(calls, 2, "{case}");
            assert_eq!(complete["work:a"], expected, "{case}");
            assert_eq!(
                readings["work:a"].len(),
                before.unwrap()[&10].len(),
                "{case}"
            );
        }
    }

    #[test]
    fn conflicting_native_identity_marks_both_scopes_unavailable() {
        let a = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let b = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("b").unwrap());
        let groups = vec![group(b.clone(), "b-2"), group(a.clone(), "b-1")];
        let identity = ProcessIdentity {
            pid: 10,
            start_time: 1,
        };
        let identities = BTreeMap::from([(10, BTreeMap::from([(10, identity)]))]);
        let (members, complete) = scope_cpu_members(&groups, Some(&identities));
        assert_eq!(members["work:a"].len(), 1);
        assert_eq!(members["work:b"].len(), 1);
        assert!(!complete["work:a"]);
        assert!(!complete["work:b"]);
        let reversed = groups.into_iter().rev().collect::<Vec<_>>();
        assert_eq!(scope_cpu_members(&reversed, Some(&identities)).1, complete);

        let mut changed = group(b, "b-2");
        changed.launch_identity.process.start_time = 99;
        let (members, complete) = scope_cpu_members(&[group(a, "b-1"), changed], Some(&identities));
        assert!(!complete["work:a"]);
        assert!(!complete["work:b"]);
        assert!(members["work:b"].is_empty());
    }

    #[test]
    fn missing_launch_leader_does_not_adopt_other_cpu_group_members() {
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let groups = vec![group(scope, "b-1")]; // launch pid 10, start 1
        let identities = BTreeMap::from([(
            10,
            BTreeMap::from([(
                11,
                ProcessIdentity {
                    pid: 11,
                    start_time: 2,
                },
            )]),
        )]);
        let (members, complete) = scope_cpu_members(&groups, Some(&identities));
        assert!(members["work:a"].is_empty());
        assert!(!complete["work:a"]);
    }

    #[test]
    fn exited_scope_keeps_window_but_latest_and_memory_are_zero() {
        let now = Instant::now();
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let mut state = CpuState::default();
        state.intervals.push_back(CpuInterval {
            end: now,
            elapsed: Duration::from_secs(5),
            cores: BTreeMap::from([("work:a".into(), Some(1.0))]),
        });
        let result = summarize(&scope, &[], None, None, &state, now, Utc::now(), true);
        assert_eq!(result.cpu_cores_latest, Some(0.0));
        assert_eq!(result.cpu_sampled_at, None);
        assert_eq!(result.cpu_one_minute.average_cores, Some(1.0));
        assert!((result.cpu_one_minute.sampled_seconds - 5.0).abs() < f64::EPSILON);
        assert_eq!(result.memory_bytes, Some(0));
    }

    #[test]
    fn cpu_timestamp_is_owned_by_cpu_point_not_memory_observation() {
        let now = Instant::now();
        let scope =
            ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let groups = vec![group(scope.clone(), "b-1")];
        let cpu_at = Utc::now() - chrono::Duration::seconds(3);
        let mut state = CpuState {
            previous: Some(CpuPoint {
                at: now.checked_sub(Duration::from_secs(3)).unwrap(),
                sampled_at: cpu_at,
                readings: BTreeMap::new(),
                complete: BTreeMap::new(),
                launches: BTreeMap::from([(
                    scope.stable_key(),
                    BTreeSet::from([groups[0].stable_resource_identity()]),
                )]),
                lifecycle_epochs: BTreeMap::from([(scope.stable_key(), Some(1))]),
            }),
            ..CpuState::default()
        };
        state.latest.insert(scope.stable_key(), Some(0.5));
        let memory_at = Utc::now();
        let result = summarize(&scope, &groups, Some(1), None, &state, now, memory_at, true);
        assert_eq!(result.sampled_at, memory_at);
        assert_eq!(result.cpu_sampled_at, Some(cpu_at));
        assert_eq!(result.cpu_cores_latest, Some(0.5));
        assert_eq!(
            summarize(
                &scope,
                &groups,
                Some(1),
                None,
                &state,
                now + CADENCE * 3,
                memory_at,
                true,
            )
            .cpu_sampled_at,
            None
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn memory_gaps_are_scoped_not_host_wide() {
        use super::super::deployment::HostResources;
        use super::super::process_sample::ProcessObservation;
        use super::super::resource_monitor::HandleObservationTarget;
        let now = Instant::now();
        let a = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("a").unwrap());
        let b = ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("b").unwrap());
        let groups = vec![group(a.clone(), "b-1"), group(b.clone(), "b-2")];
        let mut observation = ResourceObservationGeneration {
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
            observations: BTreeMap::from([(
                10,
                ProcessObservation {
                    pid: 10,
                    name: "test".into(),
                    cpu_percent: None,
                    memory_bytes: Some(123),
                    thread_count: None,
                    cpu_time_seconds: None,
                },
            )]),
            api_pids: BTreeSet::new(),
            terminal_pids: None,
            handles: vec![HandleObservationTarget {
                scope_key: a.stable_key(),
                handle_id: "b-1".into(),
                pids: BTreeSet::from([10]),
            }],
            incomplete_bash_scopes: BTreeSet::from([b.stable_key()]),
        };
        let state = CpuState::default();
        let a_summary = summarize(
            &a,
            &groups,
            None,
            Some(&observation),
            &state,
            now,
            Utc::now(),
            true,
        );
        let b_summary = summarize(
            &b,
            &groups,
            None,
            Some(&observation),
            &state,
            now,
            Utc::now(),
            true,
        );
        assert_eq!(
            (a_summary.memory_bytes, a_summary.process_count),
            (Some(123), Some(1))
        );
        assert_eq!(
            (b_summary.memory_bytes, b_summary.process_count),
            (None, None)
        );
        let before_claim = LiveHandleSnapshot {
            groups: vec![groups[0].clone()],
            lifecycle_epochs: std::collections::HashMap::from([
                (a.clone(), Some(1)),
                (b.clone(), Some(0)),
            ]),
        };
        let after_claim = LiveHandleSnapshot {
            groups: groups.clone(),
            lifecycle_epochs: std::collections::HashMap::from([
                (a.clone(), Some(1)),
                (b.clone(), Some(1)),
            ]),
        };
        for scope in [&a, &b] {
            let summary = summarize(
                scope,
                &after_claim.groups,
                stable_scope_epoch(&before_claim, &after_claim, scope),
                Some(&observation),
                &state,
                now,
                Utc::now(),
                memory_snapshot_stable(&before_claim, &after_claim, scope),
            );
            assert_eq!((summary.memory_bytes, summary.process_count), (None, None));
        }
        observation.handles.push(HandleObservationTarget {
            scope_key: b.stable_key(),
            handle_id: "b-2".into(),
            pids: BTreeSet::from([10]),
        });
        assert_eq!(
            summarize(
                &a,
                &groups,
                None,
                Some(&observation),
                &state,
                now,
                Utc::now(),
                true,
            )
            .memory_bytes,
            None
        );
        observation.incomplete_bash_scopes.clear();
        assert_eq!(
            summarize(
                &b,
                &groups,
                None,
                Some(&observation),
                &state,
                now,
                Utc::now(),
                true,
            )
            .memory_bytes,
            None
        );
        observation.handles.pop();
        observation.observations.clear();
        assert_eq!(
            summarize(
                &a,
                &groups,
                None,
                Some(&observation),
                &state,
                now,
                Utc::now(),
                true,
            )
            .process_count,
            None
        );
    }
}
