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

#[cfg(target_os = "macos")]
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};
use tokio::sync::Mutex;

const FRESHNESS_LEASE: Duration = Duration::from_millis(1_200);
const SAFE_PRESSURE_DURATION: chrono::Duration = chrono::Duration::milliseconds(1_200);
const MAX_SAFE_OBSERVATION_GAP: chrono::Duration = chrono::Duration::milliseconds(1_200);

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
    pub thermal: ThermalObservationSample,
    pub thermal_decision: ThermalGovernorDecisionSample,
    pub observations: BTreeMap<u32, ProcessObservation>,
    pub api_pids: BTreeSet<u32>,
    pub terminal_pids: Option<BTreeSet<u32>>,
    pub handles: Vec<HandleObservationTarget>,
    pub live_bash_work_scopes: BTreeSet<String>,
    pub covered_bash_identities: BTreeSet<ProcessIdentity>,
    pub bash_sample_failure_count: u32,
    pub bash_attribution_available: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalPressureSample {
    Nominal,
    Elevated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ThermalProviderUnavailableReason {
    UnsupportedPlatform,
    ProviderFailure,
    Unreadable,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThermalObservationSample {
    Available {
        pressure: ThermalPressureSample,
        sampled_at: DateTime<Utc>,
    },
    Unavailable {
        reason: ThermalProviderUnavailableReason,
        sampled_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalGovernorStateSample {
    Nominal,
    Elevated,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalGovernorActionSample {
    None,
    Deprioritize,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedThermalSample {
    Fresh {
        pressure: ThermalPressureSample,
        sampled_at: DateTime<Utc>,
    },
    Stale {
        pressure: ThermalPressureSample,
        sampled_at: DateTime<Utc>,
        latest_attempted_at: DateTime<Utc>,
        reason: ThermalProviderUnavailableReason,
    },
    Unavailable {
        attempted_at: DateTime<Utc>,
        reason: ThermalProviderUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalGovernorDecisionSample {
    pub state: ThermalGovernorStateSample,
    pub proposed_action: ThermalGovernorActionSample,
    pub proposed_targets: BTreeSet<ProcessIdentity>,
    pub reported_sample: ReportedThermalSample,
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

struct ThermalDecisionState {
    state: ThermalGovernorStateSample,
    nominal_since: Option<DateTime<Utc>>,
    last_observed_at: Option<DateTime<Utc>>,
    proposed_action: ThermalGovernorActionSample,
    proposed_targets: BTreeSet<ProcessIdentity>,
    hypothetically_deprioritized: BTreeSet<ProcessIdentity>,
    last_good_sample: Option<(ThermalPressureSample, DateTime<Utc>)>,
}

impl Default for ThermalDecisionState {
    fn default() -> Self {
        Self {
            state: ThermalGovernorStateSample::Nominal,
            nominal_since: None,
            last_observed_at: None,
            proposed_action: ThermalGovernorActionSample::None,
            proposed_targets: BTreeSet::new(),
            hypothetically_deprioritized: BTreeSet::new(),
            last_good_sample: None,
        }
    }
}

impl ThermalDecisionState {
    fn observe(
        &mut self,
        sample: &ThermalObservationSample,
        covered_targets: &BTreeSet<ProcessIdentity>,
    ) -> ThermalGovernorDecisionSample {
        let previous_state = self.state;
        let reported_sample = match *sample {
            ThermalObservationSample::Available {
                pressure,
                sampled_at,
            } => {
                self.last_good_sample = Some((pressure, sampled_at));
                ReportedThermalSample::Fresh {
                    pressure,
                    sampled_at,
                }
            }
            ThermalObservationSample::Unavailable { reason, sampled_at } => {
                match self.last_good_sample {
                    Some((pressure, last_good_at)) => ReportedThermalSample::Stale {
                        pressure,
                        sampled_at: last_good_at,
                        latest_attempted_at: sampled_at,
                        reason,
                    },
                    None => ReportedThermalSample::Unavailable {
                        attempted_at: sampled_at,
                        reason,
                    },
                }
            }
        };

        match sample {
            ThermalObservationSample::Available {
                pressure: ThermalPressureSample::Elevated,
                ..
            } => {
                self.nominal_since = None;
                self.state = ThermalGovernorStateSample::Elevated;
                self.proposed_action = ThermalGovernorActionSample::Deprioritize;
                self.proposed_targets.clone_from(covered_targets);
                self.hypothetically_deprioritized
                    .extend(covered_targets.iter().copied());
            }
            ThermalObservationSample::Available {
                pressure: ThermalPressureSample::Nominal,
                sampled_at,
            } if self.state == ThermalGovernorStateSample::Elevated => {
                let gap_is_contiguous = self.last_observed_at.is_some_and(|last| {
                    let gap = *sampled_at - last;
                    gap >= chrono::Duration::zero() && gap <= MAX_SAFE_OBSERVATION_GAP
                });
                if !gap_is_contiguous {
                    self.nominal_since = Some(*sampled_at);
                }
                let nominal_since = self.nominal_since.get_or_insert(*sampled_at);
                if *sampled_at - *nominal_since >= SAFE_PRESSURE_DURATION {
                    self.state = ThermalGovernorStateSample::Nominal;
                    self.nominal_since = None;
                    self.proposed_action = ThermalGovernorActionSample::Restore;
                    self.proposed_targets = std::mem::take(&mut self.hypothetically_deprioritized);
                }
            }
            ThermalObservationSample::Available {
                pressure: ThermalPressureSample::Nominal,
                ..
            } => {
                self.nominal_since = None;
                self.proposed_action = ThermalGovernorActionSample::None;
                self.proposed_targets.clear();
                self.hypothetically_deprioritized.clear();
                if self.state == ThermalGovernorStateSample::Unavailable {
                    self.state = ThermalGovernorStateSample::Nominal;
                }
            }
            ThermalObservationSample::Unavailable { .. } => {
                self.nominal_since = None;
                if self.last_good_sample.is_none() {
                    self.state = ThermalGovernorStateSample::Unavailable;
                }
            }
        }
        self.last_observed_at = match sample {
            ThermalObservationSample::Available { sampled_at, .. } => Some(*sampled_at),
            ThermalObservationSample::Unavailable { .. } => None,
        };
        if self.state != previous_state {
            tracing::debug!(
                previous_state = ?previous_state,
                state = ?self.state,
                proposed_action = ?self.proposed_action,
                mode = "observe_only",
                "macOS thermal governor state changed"
            );
        }
        ThermalGovernorDecisionSample {
            state: self.state,
            proposed_action: self.proposed_action,
            proposed_targets: self.proposed_targets.clone(),
            reported_sample,
        }
    }
}

#[derive(Default)]
struct MonitorState {
    cached: Option<(ObservationKey, Instant, Arc<ResourceObservationGeneration>)>,
    sample_count: u64,
    thermal_decision: ThermalDecisionState,
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

        let mut generation = sample().await;
        generation.thermal_decision = state
            .thermal_decision
            .observe(&generation.thermal, &generation.covered_bash_identities);
        let generation = Arc::new(generation);
        state.sample_count = state.sample_count.saturating_add(1);
        state.cached = Some((key, Instant::now(), generation.clone()));
        generation
    }

    #[cfg(test)]
    pub async fn sample_count(&self) -> u64 {
        self.state.lock().await.sample_count
    }
}

fn sample_thermal_observation() -> ThermalObservationSample {
    #[cfg(target_os = "macos")]
    {
        let sampled_at = Utc::now();
        let process_info = NSProcessInfo::processInfo();
        let pressure = match process_info.thermalState() {
            NSProcessInfoThermalState::Nominal => Some(ThermalPressureSample::Nominal),
            NSProcessInfoThermalState::Fair
            | NSProcessInfoThermalState::Serious
            | NSProcessInfoThermalState::Critical => Some(ThermalPressureSample::Elevated),
            other => {
                tracing::debug!(state = ?other, "thermal provider returned unreadable state");
                None
            }
        };
        match pressure {
            Some(pressure) => ThermalObservationSample::Available {
                pressure,
                sampled_at,
            },
            None => ThermalObservationSample::Unavailable {
                reason: ThermalProviderUnavailableReason::Unreadable,
                sampled_at,
            },
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        ThermalObservationSample::Unavailable {
            reason: ThermalProviderUnavailableReason::UnsupportedPlatform,
            sampled_at: Utc::now(),
        }
    }
}

fn bash_sampling_coverage(
    expected: &BTreeMap<u32, ProcessIdentity>,
    rows: &[ProcessObservation],
) -> (BTreeSet<ProcessIdentity>, u32) {
    let observed_pids = rows.iter().map(|row| row.pid).collect::<BTreeSet<_>>();
    let covered = expected
        .iter()
        .filter_map(|(pid, identity)| observed_pids.contains(pid).then_some(*identity))
        .collect::<BTreeSet<_>>();
    let failures = u32::try_from(expected.len().saturating_sub(covered.len())).unwrap_or(u32::MAX);
    (covered, failures)
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
    let live_bash_work_scopes = live_groups
        .iter()
        .map(|group| group.work_scope.stable_key())
        .collect();
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
    all_identities.extend(
        bash_identities
            .iter()
            .map(|(pid, identity)| (*pid, *identity)),
    );
    if let Some(identities) = &terminal_identities {
        all_identities.extend(identities.clone());
    }

    let thermal = sample_thermal_observation();
    let (host, rows) = tokio::join!(
        super::deployment::sample_host_resources(),
        sample_process_observations(&all_identities)
    );

    let reported_sample = match thermal {
        ThermalObservationSample::Available {
            pressure,
            sampled_at,
        } => ReportedThermalSample::Fresh {
            pressure,
            sampled_at,
        },
        ThermalObservationSample::Unavailable { reason, sampled_at } => {
            ReportedThermalSample::Unavailable {
                attempted_at: sampled_at,
                reason,
            }
        }
    };
    let (covered_bash_identities, bash_sample_failure_count) =
        bash_sampling_coverage(&bash_identities, &rows);
    if bash_sample_failure_count > 0 {
        tracing::debug!(
            failed_processes = bash_sample_failure_count,
            "Bash process identity disappeared or changed during resource sampling"
        );
    }
    ResourceObservationGeneration {
        sampled_at: Utc::now(),
        host,
        thermal,
        thermal_decision: ThermalGovernorDecisionSample {
            state: ThermalGovernorStateSample::Unavailable,
            proposed_action: ThermalGovernorActionSample::None,
            proposed_targets: BTreeSet::new(),
            reported_sample,
        },
        observations: rows.into_iter().map(|row| (row.pid, row)).collect(),
        api_pids: api_identities.keys().copied().collect(),
        terminal_pids: terminal_identities.map(|ids| ids.keys().copied().collect()),
        handles,
        live_bash_work_scopes,
        covered_bash_identities,
        bash_sample_failure_count,
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
            thermal: ThermalObservationSample::Unavailable {
                reason: ThermalProviderUnavailableReason::UnsupportedPlatform,
                sampled_at: Utc::now(),
            },
            thermal_decision: ThermalGovernorDecisionSample {
                state: ThermalGovernorStateSample::Unavailable,
                proposed_action: ThermalGovernorActionSample::None,
                proposed_targets: BTreeSet::new(),
                reported_sample: ReportedThermalSample::Unavailable {
                    attempted_at: Utc::now(),
                    reason: ThermalProviderUnavailableReason::UnsupportedPlatform,
                },
            },
            observations: BTreeMap::new(),
            api_pids: BTreeSet::new(),
            terminal_pids: Some(BTreeSet::new()),
            handles: Vec::new(),
            live_bash_work_scopes: BTreeSet::new(),
            covered_bash_identities: BTreeSet::new(),
            bash_sample_failure_count: 0,
            bash_attribution_available: true,
        }
    }

    #[test]
    fn bash_sampling_coverage_reports_post_enumeration_identity_loss() {
        let retained = ProcessIdentity {
            pid: 10,
            start_time: 100,
        };
        let disappeared = ProcessIdentity {
            pid: 11,
            start_time: 200,
        };
        let expected = BTreeMap::from([(10, retained), (11, disappeared)]);
        let rows = vec![ProcessObservation {
            pid: 10,
            name: "retained".into(),
            cpu_percent: None,
            memory_bytes: None,
            thread_count: None,
            cpu_time_seconds: None,
        }];

        let (covered, failures) = bash_sampling_coverage(&expected, &rows);

        assert_eq!(covered, BTreeSet::from([retained]));
        assert_eq!(failures, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_thermal_provider_returns_public_pressure_state() {
        assert!(matches!(
            sample_thermal_observation(),
            ThermalObservationSample::Available { .. }
        ));
    }

    #[test]
    fn thermal_decision_requires_sustained_nominal_time_before_restore() {
        let now = Utc::now();
        let elevated = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Elevated,
            sampled_at: now,
        };
        let nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(1),
        };
        let sustained_nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(1_201),
        };
        let mut state = ThermalDecisionState::default();

        let decision = state.observe(&elevated, &BTreeSet::new());
        assert_eq!(decision.state, ThermalGovernorStateSample::Elevated);
        assert_eq!(
            decision.proposed_action,
            ThermalGovernorActionSample::Deprioritize
        );
        let repeated = state.observe(&elevated, &BTreeSet::new());
        assert_eq!(repeated.state, ThermalGovernorStateSample::Elevated);
        assert_eq!(
            repeated.proposed_action,
            ThermalGovernorActionSample::Deprioritize
        );
        let first_safe = state.observe(&nominal, &BTreeSet::new());
        assert_eq!(first_safe.state, ThermalGovernorStateSample::Elevated);
        assert_eq!(
            first_safe.proposed_action,
            ThermalGovernorActionSample::Deprioritize
        );
        let restored = state.observe(&sustained_nominal, &BTreeSet::new());
        assert_eq!(restored.state, ThermalGovernorStateSample::Nominal);
        assert_eq!(
            restored.proposed_action,
            ThermalGovernorActionSample::Restore
        );
        let settled = state.observe(&nominal, &BTreeSet::new());
        assert_eq!(settled.state, ThermalGovernorStateSample::Nominal);
        assert_eq!(settled.proposed_action, ThermalGovernorActionSample::None);
    }

    #[test]
    fn provider_outage_retains_elevated_latch_and_last_good_sample() {
        let now = Utc::now();
        let elevated = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Elevated,
            sampled_at: now,
        };
        let unavailable = ThermalObservationSample::Unavailable {
            reason: ThermalProviderUnavailableReason::ProviderFailure,
            sampled_at: now + chrono::Duration::seconds(1),
        };
        let nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::seconds(2),
        };
        let sustained_nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(3_200),
        };
        let mut state = ThermalDecisionState::default();

        state.observe(&elevated, &BTreeSet::new());
        let outage = state.observe(&unavailable, &BTreeSet::new());
        assert_eq!(outage.state, ThermalGovernorStateSample::Elevated);
        assert_eq!(
            outage.proposed_action,
            ThermalGovernorActionSample::Deprioritize
        );
        assert!(matches!(
            outage.reported_sample,
            ReportedThermalSample::Stale {
                pressure: ThermalPressureSample::Elevated,
                sampled_at,
                ..
            } if sampled_at == now
        ));
        assert_eq!(
            state.observe(&nominal, &BTreeSet::new()).state,
            ThermalGovernorStateSample::Elevated
        );
        let restored = state.observe(&sustained_nominal, &BTreeSet::new());
        assert_eq!(restored.state, ThermalGovernorStateSample::Nominal);
        assert_eq!(
            restored.proposed_action,
            ThermalGovernorActionSample::Restore
        );
    }

    #[test]
    fn long_gap_restarts_safe_pressure_duration() {
        let now = Utc::now();
        let elevated = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Elevated,
            sampled_at: now,
        };
        let first_nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(1),
        };
        let after_gap = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::seconds(10),
        };
        let sustained_after_gap = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(11_200),
        };
        let mut state = ThermalDecisionState::default();

        state.observe(&elevated, &BTreeSet::new());
        state.observe(&first_nominal, &BTreeSet::new());
        let not_restored = state.observe(&after_gap, &BTreeSet::new());
        assert_eq!(not_restored.state, ThermalGovernorStateSample::Elevated);
        let restored = state.observe(&sustained_after_gap, &BTreeSet::new());
        assert_eq!(restored.state, ThermalGovernorStateSample::Nominal);
        assert_eq!(
            restored.proposed_action,
            ThermalGovernorActionSample::Restore
        );
    }

    #[test]
    fn restoration_targets_only_identities_hypothetically_deprioritized() {
        let now = Utc::now();
        let elevated = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Elevated,
            sampled_at: now,
        };
        let first_nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(1),
        };
        let sustained_nominal = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(1_201),
        };
        let original = ProcessIdentity {
            pid: 10,
            start_time: 100,
        };
        let replacement = ProcessIdentity {
            pid: 11,
            start_time: 200,
        };
        let mut state = ThermalDecisionState::default();

        let deprioritize = state.observe(&elevated, &BTreeSet::from([original]));
        assert_eq!(deprioritize.proposed_targets, BTreeSet::from([original]));
        state.observe(&first_nominal, &BTreeSet::from([replacement]));
        let membership_changed = ThermalObservationSample::Available {
            pressure: ThermalPressureSample::Nominal,
            sampled_at: now + chrono::Duration::milliseconds(2),
        };
        let still_elevated = state.observe(&membership_changed, &BTreeSet::from([replacement]));
        assert_eq!(still_elevated.state, ThermalGovernorStateSample::Elevated);
        let restore = state.observe(&sustained_nominal, &BTreeSet::from([replacement]));

        assert_eq!(
            restore.proposed_action,
            ThermalGovernorActionSample::Restore
        );
        assert_eq!(restore.proposed_targets, BTreeSet::from([original]));
        assert_eq!(state.proposed_targets, BTreeSet::from([original]));
        assert!(state.hypothetically_deprioritized.is_empty());
    }

    #[test]
    fn unavailable_thermal_sample_never_proposes_native_policy() {
        let mut state = ThermalDecisionState::default();
        let unavailable = ThermalObservationSample::Unavailable {
            reason: ThermalProviderUnavailableReason::ProviderFailure,
            sampled_at: Utc::now(),
        };

        let decision = state.observe(&unavailable, &BTreeSet::new());
        assert_eq!(decision.state, ThermalGovernorStateSample::Unavailable);
        assert_eq!(decision.proposed_action, ThermalGovernorActionSample::None);
        assert!(matches!(
            decision.reported_sample,
            ReportedThermalSample::Unavailable { .. }
        ));
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
            generation.visible_handle_pids("conversation:c1", &["b-1".into()]),
            BTreeSet::from([10, 11])
        );
        assert_eq!(generation.all_bash_pids(), BTreeSet::from([10, 11, 12]));
        assert_eq!(
            generation.handle_pids("conversation:c1", "b-1"),
            Some(&BTreeSet::from([10, 11]))
        );
    }
}
