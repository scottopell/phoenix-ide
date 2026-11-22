//! Native initial process-retirement ownership and observation.
//!
//! Production uses exact prepared resources, synchronous source-writer transfer,
//! confirmed release and externally retained supervision in `resource`. The
//! legacy kind-wide adapter below is test-only, not a production entry path.

pub(crate) mod resource;
pub(crate) mod bash;
pub(super) mod capture_binding;
mod supervision;

use phoenix_core::domain::close::{
    CloseRetirementSnapshot, LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity,
    RetiredResourceKind, RetirementOutcome,
};
use phoenix_db::{CloseProcessResourceKind, CloseProcessStepOutcome};
use super::{CloseProcessStepExecutor, RetirementFailureReason};
use super::verify_observed_process_identities;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeInitialProcessStep {
    Bash,
    Tmux,
    Terminal,
    Browser,
}

impl NativeInitialProcessStep {
    /// Existing initial-retirement vocabulary/order; callers choose exactly one.
    pub(crate) const ORDERED: [Self; 4] = [Self::Bash, Self::Tmux, Self::Terminal, Self::Browser];

    pub(crate) const fn retired_kind(self) -> RetiredResourceKind {
        match self {
            Self::Bash => RetiredResourceKind::BashProcessGroup,
            Self::Tmux => RetiredResourceKind::TmuxServer,
            Self::Terminal => RetiredResourceKind::PtySession,
            Self::Browser => RetiredResourceKind::BrowserSession,
        }
    }

    const fn process_kind(self) -> Option<CloseProcessResourceKind> {
        match self {
            Self::Bash => Some(CloseProcessResourceKind::BashProcessGroup),
            Self::Tmux => None,
            Self::Terminal => Some(CloseProcessResourceKind::PtySession),
            Self::Browser => Some(CloseProcessResourceKind::BrowserSession),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeInitialObservation {
    Process {
        kind: CloseProcessResourceKind,
        identities: Vec<OpaqueIdentity>,
        outcome: CloseProcessStepOutcome,
        observed_at_us: i64,
    },
    Tmux {
        resources: Vec<RetiredResourceIdentity>,
        outcome: RetirementOutcome,
        observed_at_us: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeInitialStepError {
    FatalAuthority,
    MissingLease,
    AlreadyEntered,
    SealedInventoryMismatch,
    InvalidSealedInventory(String),
    /// Executor-provided evidence is preserved verbatim; it is never inferred.
    ProcessFailure {
        kind: RetiredResourceKind,
        resources: Vec<RetiredResourceIdentity>,
        detail: String,
    },
    /// The tmux executor currently supplies no resource-attributed failure evidence.
    Unattributed {
        kind: RetiredResourceKind,
        reason: RetirementFailureReason,
        detail: String,
    },
    ObservationMismatch {
        kind: RetiredResourceKind,
        detail: String,
    },
}

fn expected_resources<'a>(
    inventory: &'a [RetiredResourceIdentity],
    kind: RetiredResourceKind,
) -> Vec<&'a RetiredResourceIdentity> {
    inventory.iter().filter(|resource| resource.kind() == kind).collect()
}

fn inventories_match(
    left: &[RetiredResourceIdentity],
    right: &[RetiredResourceIdentity],
) -> bool {
    left.len() == right.len() && left.iter().all(|resource| right.contains(resource))
}

fn validate_inventory(inventory: &[RetiredResourceIdentity]) -> Result<(), String> {
    let mut seen = Vec::new();
    for resource in inventory {
        let allowed = matches!(
            resource.kind(),
            RetiredResourceKind::BashProcessGroup
                | RetiredResourceKind::TmuxServer
                | RetiredResourceKind::PtySession
                | RetiredResourceKind::BrowserSession
        );
        if !allowed {
            return Err(format!("unsupported sealed native process kind: {:?}", resource.kind()));
        }
        if seen.contains(&resource) {
            return Err("sealed inventory contains duplicate resource identities".into());
        }
        if !matches!(resource.identity(), LossItemIdentity::Opaque(_)) {
            return Err("sealed native process identity is not opaque".into());
        }
        seen.push(resource);
    }
    Ok(())
}

fn verify_process_observation(
    observed: &[OpaqueIdentity],
    expected: &[&RetiredResourceIdentity],
) -> Result<(), String> {
    let expected = expected
        .iter()
        .map(|resource| match resource.identity() {
            LossItemIdentity::Opaque(identity) => Ok(identity.as_str().to_string()),
            _ => Err("expected process identity is not opaque".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    verify_observed_process_identities(observed, &expected)
}

fn verify_tmux_observation(
    observed: &[RetiredResourceIdentity],
    expected: &[&RetiredResourceIdentity],
) -> Result<(), String> {
    let expected = expected.iter().map(|resource| (*resource).clone()).collect::<Vec<_>>();
    let actual = observed.to_vec();
    if actual.iter().enumerate().any(|(index, resource)| actual[..index].contains(resource))
        || expected.iter().enumerate().any(|(index, resource)| expected[..index].contains(resource))
    {
        return Err("tmux observation or sealed inventory contains duplicate resources".into());
    }
    if actual.len() != expected.len() || actual.iter().any(|resource| !expected.contains(resource)) {
        return Err(format!("observed tmux resources {actual:?} do not match sealed resources {expected:?}"));
    }
    Ok(())
}

// This is process-epoch custody only: it does not synchronize with source
// revocation or authorize nested/stale-source work.
fn enter_native_initial_kind(
    lease_resources: &[RetiredResourceIdentity],
    lease_snapshot: Option<&CloseRetirementSnapshot>,
    entered: &mut Vec<RetiredResourceKind>,
    snapshot: &CloseRetirementSnapshot,
    step: NativeInitialProcessStep,
    sealed_inventory: &[RetiredResourceIdentity],
) -> Result<(), NativeInitialStepError> {
    if lease_snapshot != Some(snapshot) {
        return Err(NativeInitialStepError::SealedInventoryMismatch);
    }
    validate_inventory(lease_resources).map_err(NativeInitialStepError::InvalidSealedInventory)?;
    validate_inventory(sealed_inventory).map_err(NativeInitialStepError::InvalidSealedInventory)?;
    let kind = step.retired_kind();
    let leased_kind = lease_resources.iter().filter(|resource| resource.kind() == kind)
        .cloned().collect::<Vec<_>>();
    if sealed_inventory.iter().any(|resource| resource.kind() != kind)
        || !inventories_match(&leased_kind, sealed_inventory)
    {
        return Err(NativeInitialStepError::SealedInventoryMismatch);
    }
    if entered.contains(&kind) {
        return Err(NativeInitialStepError::AlreadyEntered);
    }
    entered.push(kind);
    Ok(())
}

async fn execute_one_kind<E: CloseProcessStepExecutor>(
    executor: &mut E,
    step: NativeInitialProcessStep,
    sealed_inventory: &[RetiredResourceIdentity],
) -> Result<NativeInitialObservation, NativeInitialStepError> {
    let kind = step.retired_kind();
    let expected = expected_resources(sealed_inventory, kind);
    if let Some(process_kind) = step.process_kind() {
        let result = match step {
            NativeInitialProcessStep::Bash => executor.retire_bash().await,
            NativeInitialProcessStep::Terminal => executor.retire_terminal().await,
            NativeInitialProcessStep::Browser => executor.retire_browser().await,
            NativeInitialProcessStep::Tmux => unreachable!("tmux has its own result type"),
        };
        let observed = result.map_err(|failure| NativeInitialStepError::ProcessFailure {
            kind,
            resources: failure.resources,
            detail: failure.reason,
        })?;
        verify_process_observation(&observed.identities, &expected).map_err(|detail| {
            NativeInitialStepError::ObservationMismatch { kind, detail }
        })?;
        return Ok(NativeInitialObservation::Process {
            kind: process_kind,
            identities: observed.identities,
            outcome: observed.outcome,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
        });
    }
    let observed = executor.retire_tmux().await.map_err(|(reason, detail)| {
        NativeInitialStepError::Unattributed { kind, reason, detail }
    })?;
    verify_tmux_observation(&observed.resources, &expected).map_err(|detail| {
        NativeInitialStepError::ObservationMismatch { kind, detail }
    })?;
    Ok(NativeInitialObservation::Tmux {
        resources: observed.resources,
        outcome: observed.outcome,
        observed_at_us: chrono::Utc::now().timestamp_micros(),
    })
}

impl super::RuntimeManager {
    /// Reads the conservative common expiry of the complete original captured
    /// scope set before the first process reservation. Every binding must match
    /// the sealed aggregate; a missing/substituted scope is not a fresh budget.
    /// Each physical owner also enforces its own original deadline.
    pub(crate) async fn native_initial_original_entry_deadline(
        &self,
        attempt: &str,
        scopes: &[phoenix_core::work_scope::WorkScopeId],
        snapshot: &phoenix_core::domain::close::CloseRetirementSnapshot,
    ) -> Result<tokio::time::Instant, NativeInitialStepError> {
        let leases = self.close_retirement_leases.lock().await;
        if scopes.is_empty() || scopes.iter().collect::<std::collections::BTreeSet<_>>().len() != scopes.len()
            || leases.complete.keys().filter(|(candidate, _)| candidate == attempt).count() != scopes.len() {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        }
        let mut deadline = None;
        for scope in scopes {
            let lease = leases.complete.get(&(attempt.to_string(), scope.clone()))
                .ok_or(NativeInitialStepError::MissingLease)?;
            let bound = lease.capture_evidence.as_ref()
                .ok_or(NativeInitialStepError::MissingLease)?;
            if !bound.verifies_scope(scope, snapshot) {
                return Err(NativeInitialStepError::SealedInventoryMismatch);
            }
            let original = bound.deadline();
            deadline = Some(deadline.map_or(original, |previous: tokio::time::Instant| previous.min(original)));
        }
        deadline.ok_or(NativeInitialStepError::MissingLease)
    }
}

#[cfg(test)]
impl super::RuntimeManager {
    /// Execute exactly one requested kind using its original still-retained lease.
    /// The lease-store mutex remains held through native completion. The kind is
    /// consumed before the first physical await, including for empty inventories;
    /// cancellation, failure, or evidence retry never clears that process-epoch bit.
    pub(crate) async fn execute_native_initial_process_step(
        &self,
        authorization: phoenix_db::native_close::NativeCloseInitialProcessAuthorization,
        browser_custody: phoenix_tools::browser::session::BrowserRetirementCustody,
    ) -> Result<NativeInitialObservation, NativeInitialStepError> {
        let (run, scope, snapshot, kind, inventory) = authorization.into_parts();
        let step = match kind {
            RetiredResourceKind::BashProcessGroup => NativeInitialProcessStep::Bash,
            RetiredResourceKind::TmuxServer => NativeInitialProcessStep::Tmux,
            RetiredResourceKind::PtySession => NativeInitialProcessStep::Terminal,
            RetiredResourceKind::BrowserSession => NativeInitialProcessStep::Browser,
            _ => return Err(NativeInitialStepError::SealedInventoryMismatch),
        };
        if inventory.iter().any(|item| item.attempt_id != run.attempt_id
            || item.scope != scope || item.snapshot != snapshot || item.resource.kind() != kind) {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        }
        let sealed_inventory = inventory.into_iter().map(|item| item.resource).collect::<Vec<_>>();
        let mut leases = self.close_retirement_leases.lock().await;
        // Lock contention must not carry a launch past fatal authority loss.
        if self.fatal_local_authority_fence.fatal_authority_loss_is_latched() {
            return Err(NativeInitialStepError::FatalAuthority);
        }
        let key = (run.attempt_id.as_str().to_string(), scope);
        let Some(lease) = leases.complete.get_mut(&key) else {
            return Err(NativeInitialStepError::MissingLease);
        };
        enter_native_initial_kind(
            &lease.resources,
            lease.capture_evidence.as_ref().map(|binding| binding.snapshot()),
            &mut lease.native_initial_entered,
            &snapshot,
            step,
            &sealed_inventory,
        )?;
        let mut executor = super::LiveCloseProcessStepExecutor { manager: self, lease, browser_custody };
        execute_one_kind(&mut executor, step, &sealed_inventory).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_vocabulary_preserves_native_initial_order_and_kind_mapping() {
        assert_eq!(
            NativeInitialProcessStep::ORDERED,
            [
                NativeInitialProcessStep::Bash,
                NativeInitialProcessStep::Tmux,
                NativeInitialProcessStep::Terminal,
                NativeInitialProcessStep::Browser,
            ]
        );
        assert_eq!(NativeInitialProcessStep::Bash.retired_kind(), RetiredResourceKind::BashProcessGroup);
        assert_eq!(NativeInitialProcessStep::Tmux.retired_kind(), RetiredResourceKind::TmuxServer);
        assert_eq!(NativeInitialProcessStep::Terminal.retired_kind(), RetiredResourceKind::PtySession);
        assert_eq!(NativeInitialProcessStep::Browser.retired_kind(), RetiredResourceKind::BrowserSession);
    }

    #[test]
    fn process_matching_is_exact_and_does_not_accept_duplicate_observations() {
        let identity = OpaqueIdentity::parse("bash:1").expect("valid identity");
        let resource = super::super::opaque_resource(RetiredResourceKind::BashProcessGroup, "bash:1".into());
        let expected = [ &resource ];
        assert!(verify_process_observation(std::slice::from_ref(&identity), &expected).is_ok());
        assert!(verify_process_observation(&[], &expected).is_err());
        assert!(verify_process_observation(&[identity.clone(), identity], &expected).is_err());
    }

    fn resource(kind: RetiredResourceKind, value: &str) -> RetiredResourceIdentity {
        super::super::opaque_resource(kind, value.to_string())
    }

    #[test]
    fn inventory_membership_ignores_order_but_rejects_missing_extra_duplicate_and_unknown() {
        let a = resource(RetiredResourceKind::BashProcessGroup, "bash:1");
        let b = resource(RetiredResourceKind::PtySession, "pty:1");
        let inventory = [a.clone(), b.clone()];
        assert!(validate_inventory(&inventory).is_ok());
        assert!(validate_inventory(&[b.clone(), a.clone()]).is_ok());
        assert!(validate_inventory(&[a.clone()]).is_ok());
        assert!(validate_inventory(&[a.clone(), a.clone()]).is_err());
        let unsupported = resource(RetiredResourceKind::EquivalentLiveResource, "unexpected:1");
        assert!(validate_inventory(&[unsupported]).is_err());

        assert!(inventories_match(&inventory, &[b.clone(), a.clone()]));
        assert!(!inventories_match(&inventory, &[a.clone()]));
        let extra = resource(RetiredResourceKind::BrowserSession, "browser:1");
        assert!(!inventories_match(&inventory, &[a.clone(), b.clone(), extra]));
        assert!(!inventories_match(&inventory, &[a.clone(), a.clone()]));
    }

    #[test]
    fn process_observation_reuses_exact_identity_verifier_without_inventing_failures() {
        let identity = OpaqueIdentity::parse("bash:exact").expect("valid identity");
        let item = resource(RetiredResourceKind::BashProcessGroup, "bash:exact");
        assert!(verify_process_observation(std::slice::from_ref(&identity), &[&item]).is_ok());
        let changed = OpaqueIdentity::parse("bash:other").expect("valid identity");
        assert!(verify_process_observation(&[changed], &[&item]).is_err());
    }

    #[derive(Default)]
    struct FakeExecutor {
        calls: Vec<&'static str>,
        fail_bash: bool,
        stall_terminal: bool,
    }

    #[async_trait::async_trait]
    impl CloseProcessStepExecutor for FakeExecutor {
        async fn retire_bash(&mut self) -> Result<super::super::CloseObservedProcessRetirement, super::super::CloseProcessStepFailure> {
            self.calls.push("bash");
            if self.fail_bash {
                return Err(super::super::CloseProcessStepFailure { resources: Vec::new(), reason: "first failure".into() });
            }
            Ok(super::super::CloseObservedProcessRetirement { identities: Vec::new(), outcome: CloseProcessStepOutcome::Retired })
        }
        async fn retire_tmux(&mut self) -> Result<super::super::CloseObservedTmuxRetirement, (RetirementFailureReason, String)> {
            self.calls.push("tmux");
            Ok(super::super::CloseObservedTmuxRetirement { resources: Vec::new(), outcome: RetirementOutcome::Retired })
        }
        async fn retire_terminal(&mut self) -> Result<super::super::CloseObservedProcessRetirement, super::super::CloseProcessStepFailure> {
            self.calls.push("terminal");
            if self.stall_terminal {
                std::future::pending::<()>().await;
            }
            Ok(super::super::CloseObservedProcessRetirement { identities: Vec::new(), outcome: CloseProcessStepOutcome::Retired })
        }
        async fn retire_browser(&mut self) -> Result<super::super::CloseObservedProcessRetirement, super::super::CloseProcessStepFailure> {
            self.calls.push("browser");
            Ok(super::super::CloseObservedProcessRetirement { identities: Vec::new(), outcome: CloseProcessStepOutcome::Retired })
        }
    }

    #[tokio::test]
    async fn one_kind_executor_calls_only_requested_step_and_stops_on_first_failure() {
        let mut executor = FakeExecutor::default();
        let result = execute_one_kind(&mut executor, NativeInitialProcessStep::Terminal, &[]).await;
        assert!(matches!(result, Ok(NativeInitialObservation::Process { kind: CloseProcessResourceKind::PtySession, .. })));
        assert_eq!(executor.calls, ["terminal"]);

        let mismatch = resource(RetiredResourceKind::BrowserSession, "browser:1");
        let mut executor = FakeExecutor::default();
        assert!(matches!(execute_one_kind(&mut executor, NativeInitialProcessStep::Browser, &[mismatch]).await, Err(NativeInitialStepError::ObservationMismatch { .. })));
        assert_eq!(executor.calls, ["browser"]);

        let mut executor = FakeExecutor { fail_bash: true, ..FakeExecutor::default() };
        let result = execute_one_kind(&mut executor, NativeInitialProcessStep::Bash, &[]).await;
        assert!(matches!(result, Err(NativeInitialStepError::ProcessFailure { detail, .. }) if detail == "first failure"));
        assert_eq!(executor.calls, ["bash"]);
    }

    async fn try_enter_and_execute(
        executor: &mut FakeExecutor,
        entered: &mut Vec<RetiredResourceKind>,
        lease_resources: &[RetiredResourceIdentity],
        snapshot: &CloseRetirementSnapshot,
        step: NativeInitialProcessStep,
        inventory: &[RetiredResourceIdentity],
    ) -> Result<NativeInitialObservation, NativeInitialStepError> {
        enter_native_initial_kind(lease_resources, Some(snapshot), entered, snapshot, step, inventory)?;
        execute_one_kind(executor, step, inventory).await
    }

    fn snapshot(value: &str) -> CloseRetirementSnapshot {
        CloseRetirementSnapshot::parse(value, "fingerprint").unwrap()
    }

    #[tokio::test]
    async fn repeated_empty_kind_is_refused_before_a_second_physical_call() {
        let snapshot = snapshot("original");
        let mut entered = Vec::new();
        let mut executor = FakeExecutor::default();
        let step = NativeInitialProcessStep::Tmux;
        assert!(try_enter_and_execute(&mut executor, &mut entered, &[], &snapshot, step, &[]).await.is_ok());
        assert!(matches!(try_enter_and_execute(&mut executor, &mut entered, &[], &snapshot, step, &[]).await,
            Err(NativeInitialStepError::AlreadyEntered)));
        assert_eq!(executor.calls, ["tmux"]);
        assert_eq!(entered, [RetiredResourceKind::TmuxServer]);
    }

    #[tokio::test]
    async fn failed_kind_cannot_run_again_or_enter_with_another_snapshot() {
        let other = snapshot("other");
        let snapshot = snapshot("original");
        let resource = resource(RetiredResourceKind::BashProcessGroup, "bash:1");
        let inventory = [resource];
        let mut entered = Vec::new();
        let mut executor = FakeExecutor { fail_bash: true, ..FakeExecutor::default() };
        let step = NativeInitialProcessStep::Bash;
        assert!(matches!(enter_native_initial_kind(&inventory, Some(&other), &mut entered, &snapshot, step, &inventory),
            Err(NativeInitialStepError::SealedInventoryMismatch)));
        assert!(matches!(enter_native_initial_kind(&inventory, None, &mut entered, &snapshot, step, &inventory),
            Err(NativeInitialStepError::SealedInventoryMismatch)));
        assert!(matches!(enter_native_initial_kind(&inventory, Some(&snapshot), &mut entered, &snapshot, step, &[]),
            Err(NativeInitialStepError::SealedInventoryMismatch)));
        assert!(entered.is_empty());
        assert!(executor.calls.is_empty());
        assert!(matches!(try_enter_and_execute(&mut executor, &mut entered, &inventory, &snapshot, step, &inventory).await,
            Err(NativeInitialStepError::ProcessFailure { .. })));
        assert!(matches!(try_enter_and_execute(&mut executor, &mut entered, &inventory, &snapshot, step, &inventory).await,
            Err(NativeInitialStepError::AlreadyEntered)));
        assert_eq!(executor.calls, ["bash"]);
    }

    #[tokio::test]
    async fn dropped_pending_physical_future_does_not_restore_entry() {
        let snapshot = snapshot("original");
        let mut entered = Vec::new();
        let mut executor = FakeExecutor { stall_terminal: true, ..FakeExecutor::default() };
        let step = NativeInitialProcessStep::Terminal;
        {
            let future = try_enter_and_execute(&mut executor, &mut entered, &[], &snapshot, step, &[]);
            tokio::pin!(future);
            assert!(matches!(futures::poll!(future.as_mut()), std::task::Poll::Pending));
        }
        assert_eq!(executor.calls, ["terminal"]);
        assert!(matches!(try_enter_and_execute(&mut executor, &mut entered, &[], &snapshot, step, &[]).await,
            Err(NativeInitialStepError::AlreadyEntered)));
        assert_eq!(executor.calls, ["terminal"]);
    }

    #[test]
    fn tmux_matching_is_exact() {
        let resource = super::super::opaque_resource(RetiredResourceKind::TmuxServer, "tmux:1".into());
        assert!(verify_tmux_observation(&[resource.clone()], &[&resource]).is_ok());
        assert!(verify_tmux_observation(&[], &[&resource]).is_err());
    }
}
