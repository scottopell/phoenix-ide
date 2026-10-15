use std::collections::BTreeMap;

use crate::identity::{
    CandidateDiscovery, ChildIdentityObservation, DiscoveryId, IdentityObservation,
    ObservationFailure, ProcessId, ProcessIdentity,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryGap {
    Observation(ObservationFailure),
    DifferentProcess(ProcessIdentity),
    NotYetObserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnresolvedDiscovery {
    pub discovery: CandidateDiscovery,
    pub gap: DiscoveryGap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedIdentityStatus {
    NeedsFreshObservation,
    Present {
        current_parent: Option<ProcessIdentity>,
    },
    VerifiedAbsent,
    Uncertain(ObservationFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedProcessTree {
    next_discovery: u64,
    identities: BTreeMap<ProcessIdentity, OwnedIdentityStatus>,
    unresolved: BTreeMap<DiscoveryId, UnresolvedDiscovery>,
}

impl OwnedProcessTree {
    #[must_use]
    pub fn new(root: ProcessIdentity) -> Self {
        Self {
            next_discovery: 0,
            identities: BTreeMap::from([(root, OwnedIdentityStatus::NeedsFreshObservation)]),
            unresolved: BTreeMap::new(),
        }
    }

    /// # Errors
    /// Returns [`UnknownParent`] unless `verified_parent` is already an owned identity.
    pub fn discover_child(
        &mut self,
        verified_parent: ProcessIdentity,
        hinted_pid: ProcessId,
    ) -> Result<CandidateDiscovery, UnknownParent> {
        if !self.identities.contains_key(&verified_parent) {
            return Err(UnknownParent);
        }
        let id = DiscoveryId::new(self.next_discovery);
        self.next_discovery = self.next_discovery.saturating_add(1);
        let discovery = CandidateDiscovery::new(id, hinted_pid, verified_parent);
        self.unresolved.insert(
            id,
            UnresolvedDiscovery {
                discovery,
                gap: DiscoveryGap::NotYetObserved,
            },
        );
        Ok(discovery)
    }

    /// # Errors
    /// Returns [`UnknownDiscovery`] unless the complete discovery epoch matches an unresolved entry.
    pub fn record_child_observation(
        &mut self,
        discovery: CandidateDiscovery,
        observation: ChildIdentityObservation,
    ) -> Result<(), UnknownDiscovery> {
        if self
            .unresolved
            .get(&discovery.id())
            .is_none_or(|unresolved| unresolved.discovery != discovery)
        {
            return Err(UnknownDiscovery);
        }

        match observation {
            ChildIdentityObservation::Stable(evidence) => {
                if evidence.discovery() != discovery {
                    return Err(UnknownDiscovery);
                }
                let identity = evidence.identity();
                self.unresolved.remove(&discovery.id());
                self.identities
                    .entry(identity)
                    .or_insert(OwnedIdentityStatus::NeedsFreshObservation);
            }
            ChildIdentityObservation::Unresolved(failure) => {
                self.unresolved
                    .get_mut(&discovery.id())
                    .expect("discovery presence checked above")
                    .gap = DiscoveryGap::Observation(failure);
            }
            ChildIdentityObservation::DifferentProcess(identity) => {
                self.unresolved
                    .get_mut(&discovery.id())
                    .expect("discovery presence checked above")
                    .gap = DiscoveryGap::DifferentProcess(identity);
            }
        }
        Ok(())
    }

    /// # Errors
    /// Returns [`UnknownOwnedIdentity`] unless `identity` was acquired as part of this owned tree.
    pub fn record_identity_observation(
        &mut self,
        identity: ProcessIdentity,
        observation: IdentityObservation,
    ) -> Result<(), UnknownOwnedIdentity> {
        let status = self
            .identities
            .get_mut(&identity)
            .ok_or(UnknownOwnedIdentity)?;
        *status = match observation {
            IdentityObservation::SameIdentity { current_parent } => {
                OwnedIdentityStatus::Present { current_parent }
            }
            IdentityObservation::ExactIdentityAbsent | IdentityObservation::PidReused(_) => {
                OwnedIdentityStatus::VerifiedAbsent
            }
            IdentityObservation::Unresolved(failure) => OwnedIdentityStatus::Uncertain(failure),
        };
        Ok(())
    }

    pub fn mark_all_for_reconciliation(&mut self) {
        for status in self.identities.values_mut() {
            *status = OwnedIdentityStatus::NeedsFreshObservation;
        }
    }

    #[must_use]
    pub fn identities(&self) -> &BTreeMap<ProcessIdentity, OwnedIdentityStatus> {
        &self.identities
    }

    #[must_use]
    pub fn unresolved(&self) -> &BTreeMap<DiscoveryId, UnresolvedDiscovery> {
        &self.unresolved
    }

    #[must_use]
    pub fn exact_signal_candidates(&self) -> Vec<ProcessIdentity> {
        self.identities
            .iter()
            .filter_map(|(identity, status)| {
                matches!(status, OwnedIdentityStatus::Present { .. })
                    .then_some(*identity)
                    .filter(|candidate| !candidate.pid().is_protected())
            })
            .collect()
    }

    #[must_use]
    pub fn cleanup_readiness(&self) -> CleanupReadiness {
        if !self.unresolved.is_empty() {
            return CleanupReadiness::UnresolvedEvidence;
        }
        if self
            .identities
            .values()
            .all(|status| *status == OwnedIdentityStatus::VerifiedAbsent)
        {
            return CleanupReadiness::VerifiedClean;
        }
        if self
            .identities
            .values()
            .any(|status| matches!(status, OwnedIdentityStatus::Uncertain(_)))
        {
            return CleanupReadiness::UnresolvedEvidence;
        }
        CleanupReadiness::OwnedIdentityMayRemain
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupReadiness {
    VerifiedClean,
    OwnedIdentityMayRemain,
    UnresolvedEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownParent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownDiscovery;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownOwnedIdentity;
