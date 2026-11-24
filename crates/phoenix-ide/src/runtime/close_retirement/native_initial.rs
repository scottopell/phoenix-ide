//! Native initial process-retirement ownership and observation.
//!
//! Production uses exact prepared resources, synchronous source-writer transfer,
//! confirmed release and externally retained supervision in `resource`.

pub(crate) mod bash;
pub(super) mod capture_binding;
pub(crate) mod resource;
mod supervision;
#[cfg(unix)]
pub(crate) mod worktree;

use phoenix_core::domain::close::{
    OpaqueIdentity, RetiredResourceIdentity, RetiredResourceKind, RetirementOutcome,
};
use phoenix_db::{CloseProcessResourceKind, CloseProcessStepOutcome};

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
    /// Executor-provided evidence is preserved verbatim; it is never inferred.
    ProcessFailure {
        kind: RetiredResourceKind,
        resources: Vec<RetiredResourceIdentity>,
        detail: String,
    },
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
        if scopes.is_empty()
            || scopes
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != scopes.len()
            || leases
                .complete
                .keys()
                .filter(|(candidate, _)| candidate == attempt)
                .count()
                != scopes.len()
        {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        }
        let mut deadline = None;
        for scope in scopes {
            let lease = leases
                .complete
                .get(&(attempt.to_string(), scope.clone()))
                .ok_or(NativeInitialStepError::MissingLease)?;
            let bound = lease
                .capture_evidence
                .as_ref()
                .ok_or(NativeInitialStepError::MissingLease)?;
            if !bound.verifies_scope(scope, snapshot) {
                return Err(NativeInitialStepError::SealedInventoryMismatch);
            }
            let original = bound.deadline();
            deadline = Some(deadline.map_or(original, |previous: tokio::time::Instant| {
                previous.min(original)
            }));
        }
        deadline.ok_or(NativeInitialStepError::MissingLease)
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
        assert_eq!(
            NativeInitialProcessStep::Bash.retired_kind(),
            RetiredResourceKind::BashProcessGroup
        );
        assert_eq!(
            NativeInitialProcessStep::Tmux.retired_kind(),
            RetiredResourceKind::TmuxServer
        );
        assert_eq!(
            NativeInitialProcessStep::Terminal.retired_kind(),
            RetiredResourceKind::PtySession
        );
        assert_eq!(
            NativeInitialProcessStep::Browser.retired_kind(),
            RetiredResourceKind::BrowserSession
        );
    }

}
