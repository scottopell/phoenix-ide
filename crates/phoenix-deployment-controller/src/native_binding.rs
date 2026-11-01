//! Durable native guardian observations. None of these values is an execution capability.
use crate::{AttemptId, MonotonicTick, PersistenceRevision, ProcessIdentity};
use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GuardianLifetimeNonce([u8; 16]);
impl GuardianLifetimeNonce {
    #[must_use]
    pub fn new(bytes: [u8; 16]) -> Option<Self> {
        (bytes != [0; 16]).then_some(Self(bytes))
    }
    #[must_use]
    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeGuardianIdentity {
    pub nonce: GuardianLifetimeNonce,
    pub process: ProcessIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSlotLiveness {
    Running,
    Exited,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeGuardianSlot {
    pub process: ProcessIdentity,
    pub liveness: NativeSlotLiveness,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSessionState {
    Active,
    Detached,
    Reconciling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeGuardianSession {
    pub generation: NonZeroU64,
    pub interruption_revision: PersistenceRevision,
    pub state: NativeSessionState,
}

/// An ordered diagnostic snapshot. The final session is the highest generation;
/// the deadline comes exclusively from the parent attempt row.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{NativeGuardianBinding, SessionDispatchTicket};
/// fn forge(binding: NativeGuardianBinding) -> SessionDispatchTicket { binding.into() }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeGuardianBinding {
    pub(crate) attempt_id: AttemptId,
    pub(crate) deadline: MonotonicTick,
    pub(crate) identity: NativeGuardianIdentity,
    pub(crate) revision: PersistenceRevision,
    pub(crate) dispatch_floor: PersistenceRevision,
    pub(crate) slots: Vec<NativeGuardianSlot>,
    pub(crate) sessions: Vec<NativeGuardianSession>,
}
impl NativeGuardianBinding {
    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt_id
    }
    #[must_use]
    pub const fn deadline(&self) -> MonotonicTick {
        self.deadline
    }
    #[must_use]
    pub const fn identity(&self) -> NativeGuardianIdentity {
        self.identity
    }
    #[must_use]
    pub const fn revision(&self) -> PersistenceRevision {
        self.revision
    }
    #[must_use]
    pub const fn dispatch_floor(&self) -> PersistenceRevision {
        self.dispatch_floor
    }
    #[must_use]
    pub fn slots(&self) -> &[NativeGuardianSlot] {
        &self.slots
    }
    #[must_use]
    pub fn sessions(&self) -> &[NativeGuardianSession] {
        &self.sessions
    }
    /// Returns the final session in generation order.
    ///
    /// # Panics
    ///
    /// Panics only if a binding is constructed without sessions. The database reader
    /// rejects an empty session history before constructing a binding.
    #[must_use]
    pub fn current_session(&self) -> &NativeGuardianSession {
        self.sessions
            .last()
            .expect("validated binding has a session")
    }
}
