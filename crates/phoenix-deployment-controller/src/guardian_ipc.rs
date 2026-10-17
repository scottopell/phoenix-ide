//! Versioned, bounded guardian transport. Wire DTOs are observations and references,
//! never executable authority. A listener must fence on connection loss and must
//! authenticate its peer before passing requests to `GuardianServer`.
use crate::{
    ArtifactDigest, BirthToken, CandidateDiscovery, ChildSlotObservation, ControllerSession,
    DiscoveryId, DispatchKey, DispatchTicket, EffectCompletion, ExecutedEffect, FullCommitSha,
    Guardian, GuardianError, GuardianExecutor, GuardianState, MonotonicTick, OsEffect,
    PersistenceRevision, ProcessId, ProcessIdentity, RetainedCandidateIdentity, Signal,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    num::NonZeroU64,
};

/// Durable evidence could not be read or validated from the pinned store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeEvidenceReadError;

/// Trusted, pinned read-only store view; wire revisions are never proof.
pub(crate) trait NativeEvidenceReader: Send {
    /// # Errors
    /// Returns an error when the pinned store cannot establish strict typed evidence.
    fn read(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(crate::LoadedAttempt, crate::NativeGuardianBinding)>, NativeEvidenceReadError>;
    /// # Errors
    /// Refuses unreadable or malformed exact reconciliation completion evidence.
    fn reconciliation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationCompletion,
        )>,
        NativeEvidenceReadError,
    >;
    fn activation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationActivation,
        )>,
        NativeEvidenceReadError,
    >;
    /// # Errors
    /// Refuses unreadable or malformed exact native sampling evidence.
    fn sampling_evidence(
        &mut self,
        key: DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeSamplingStatus,
        )>,
        NativeEvidenceReadError,
    >;
}

/// Process-local, one-use release handle. Durable rows and wire values cannot create one.
pub trait NativeInitialGate: Send {
    fn identity(&self) -> ProcessIdentity;
    /// # Errors
    /// An unsuccessful write never grants permission to retry the consumed gate.
    fn release(self: Box<Self>) -> io::Result<()>;
}

fn native_reconciliation_now() -> Result<MonotonicTick, WireError> {
    #[cfg(unix)]
    {
        MonotonicTick::native_now().map_err(|_| WireError::MissingProof)
    }
    #[cfg(not(unix))]
    {
        Err(WireError::MissingProof)
    }
}

fn before_native_deadline(deadline: u64) -> bool {
    #[cfg(unix)]
    {
        crate::MonotonicTick::native_now().is_ok_and(|now| now.get() < deadline)
    }
    #[cfg(not(unix))]
    {
        let _ = deadline;
        false
    }
}

/// Build the exact initial-release request from observed live metadata and the
/// revision whose binding the runner already committed.
///
/// # Errors
///
/// Returns `MissingProof` when the snapshot lacks native guardian evidence.
pub fn initial_release_request(
    snapshot: &WireSnapshot,
    revision: PersistenceRevision,
) -> Result<Request, WireError> {
    let binding = snapshot
        .native_binding
        .as_ref()
        .ok_or(WireError::MissingProof)?;
    Ok(Request::ReleaseInitial {
        session: snapshot.session,
        attempt: snapshot.attempt,
        nonce: binding.nonce,
        attempt_revision: revision.get(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidBuildIdentity;

/// Exact SHA-256 of the controller artifact's source inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildIdentity(String);

impl BuildIdentity {
    /// # Errors
    /// Rejects abbreviated, uppercase, and non-hexadecimal digests.
    pub fn parse(value: &str) -> Result<Self, InvalidBuildIdentity> {
        if value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidBuildIdentity)
        }
    }

    /// # Panics
    ///
    /// Panics only when this crate's build script emits an invalid digest.
    #[must_use]
    pub fn compiled() -> Self {
        Self::parse(env!("PHOENIX_SDC_BUILD_ID"))
            .expect("build script emits a validated source digest")
    }
}

impl Serialize for BuildIdentity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BuildIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(|_| serde::de::Error::custom("invalid build identity"))
    }
}

pub const PROTOCOL_VERSION: u16 = 5;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    Oversized,
    Empty,
    Malformed(serde_json::Error),
    UnsupportedVersion(u16),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    pub version: u16,
    pub body: T,
}

/// Serialize a versioned, length-prefixed frame.
///
/// # Errors
/// Returns `Malformed` if serialization fails, `Oversized` if the encoded
/// envelope exceeds the frame limit, or `Io` if either write fails.
pub fn write_frame<T: Serialize>(out: &mut impl Write, body: &T) -> Result<(), CodecError> {
    let bytes = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        body,
    })
    .map_err(CodecError::Malformed)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(CodecError::Oversized);
    }
    let len = u32::try_from(bytes.len()).map_err(|_| CodecError::Oversized)?;
    out.write_all(&len.to_be_bytes()).map_err(CodecError::Io)?;
    out.write_all(&bytes).map_err(CodecError::Io)
}

/// Read a versioned, length-prefixed frame.
///
/// # Errors
/// Returns `Io` on a failed read, `Empty` or `Oversized` for an invalid
/// length, `Malformed` for invalid JSON, or `UnsupportedVersion` for an
/// envelope with a different protocol version.
pub fn read_frame<T: for<'de> Deserialize<'de>>(input: &mut impl Read) -> Result<T, CodecError> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix).map_err(CodecError::Io)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 {
        return Err(CodecError::Empty);
    }
    if len > MAX_FRAME_BYTES {
        return Err(CodecError::Oversized);
    }
    let mut data = vec![0; len];
    input.read_exact(&mut data).map_err(CodecError::Io)?;
    let envelope: Envelope<T> = serde_json::from_slice(&data).map_err(CodecError::Malformed)?;
    if envelope.version != PROTOCOL_VERSION {
        return Err(CodecError::UnsupportedVersion(envelope.version));
    }
    Ok(envelope.body)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSession {
    pub instance: u64,
    pub generation: u64,
}
impl From<ControllerSession> for WireSession {
    fn from(value: ControllerSession) -> Self {
        Self {
            instance: value.instance_id().get(),
            generation: value.generation(),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireKey {
    pub attempt: u128,
    pub revision: u64,
    pub ordinal: u64,
}
impl TryFrom<DispatchKey> for WireKey {
    type Error = WireError;
    fn try_from(key: DispatchKey) -> Result<Self, Self::Error> {
        Ok(Self {
            attempt: key.attempt_id().get(),
            revision: key.revision().get(),
            ordinal: key
                .ordinal()
                .try_into()
                .map_err(|_| WireError::InvalidValue)?,
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSlot {
    pub instance: u64,
    pub ordinal: u64,
}
impl From<crate::ChildSlotId> for WireSlot {
    fn from(slot: crate::ChildSlotId) -> Self {
        Self {
            instance: slot.instance_id().get(),
            ordinal: slot.ordinal() as u64,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireIdentity {
    pub pid: u32,
    pub birth_seconds: u64,
    pub birth_nanos: u32,
}
impl From<ProcessIdentity> for WireIdentity {
    fn from(value: ProcessIdentity) -> Self {
        Self {
            pid: value.pid().get(),
            birth_seconds: value.birth().seconds(),
            birth_nanos: value.birth().nanos(),
        }
    }
}
impl TryFrom<WireIdentity> for ProcessIdentity {
    type Error = WireError;
    fn try_from(value: WireIdentity) -> Result<Self, Self::Error> {
        Ok(Self::new(
            ProcessId::new(value.pid.try_into().map_err(|_| WireError::InvalidValue)?),
            BirthToken::new(value.birth_seconds, value.birth_nanos)
                .map_err(|_| WireError::InvalidValue)?,
        ))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireDiscovery {
    pub id: u64,
    pub hinted_pid: u32,
    pub verified_parent: WireIdentity,
}
impl From<CandidateDiscovery> for WireDiscovery {
    fn from(value: CandidateDiscovery) -> Self {
        Self {
            id: value.id().get(),
            hinted_pid: value.hinted_pid().get(),
            verified_parent: value.verified_parent().into(),
        }
    }
}
impl TryFrom<WireDiscovery> for CandidateDiscovery {
    type Error = WireError;
    fn try_from(value: WireDiscovery) -> Result<Self, Self::Error> {
        Ok(Self::new(
            DiscoveryId::new(value.id),
            ProcessId::new(
                value
                    .hinted_pid
                    .try_into()
                    .map_err(|_| WireError::InvalidValue)?,
            ),
            value.verified_parent.try_into()?,
        ))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireEffect {
    SampleResources {
        sequence: u64,
    },
    ListChildren {
        parent: WireIdentity,
    },
    AcquireChildIdentity {
        discovery: WireDiscovery,
    },
    ObserveIdentity {
        identity: WireIdentity,
    },
    SignalExactIdentity {
        identity: WireIdentity,
        signal: WireSignal,
    },
    ReverifyRetainedCandidate {
        commit: [u8; 20],
        digest: [u8; 32],
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSignal {
    Terminate,
    Kill,
}
impl From<Signal> for WireSignal {
    fn from(signal: Signal) -> Self {
        match signal {
            Signal::Terminate => Self::Terminate,
            Signal::Kill => Self::Kill,
        }
    }
}
impl From<WireSignal> for Signal {
    fn from(signal: WireSignal) -> Self {
        match signal {
            WireSignal::Terminate => Self::Terminate,
            WireSignal::Kill => Self::Kill,
        }
    }
}
impl From<&OsEffect> for WireEffect {
    fn from(value: &OsEffect) -> Self {
        match value {
            OsEffect::SampleResources { sequence } => Self::SampleResources {
                sequence: *sequence,
            },
            OsEffect::ListChildren { parent } => Self::ListChildren {
                parent: (*parent).into(),
            },
            OsEffect::AcquireChildIdentity { discovery } => Self::AcquireChildIdentity {
                discovery: (*discovery).into(),
            },
            OsEffect::ObserveIdentity { identity } => Self::ObserveIdentity {
                identity: (*identity).into(),
            },
            OsEffect::SignalExactIdentity { identity, signal } => Self::SignalExactIdentity {
                identity: (*identity).into(),
                signal: (*signal).into(),
            },
            OsEffect::ReverifyRetainedCandidate { candidate } => Self::ReverifyRetainedCandidate {
                commit: candidate.source_commit.bytes(),
                digest: candidate.artifact_digest.bytes(),
            },
        }
    }
}
impl TryFrom<WireEffect> for OsEffect {
    type Error = WireError;
    fn try_from(value: WireEffect) -> Result<Self, Self::Error> {
        Ok(match value {
            WireEffect::SampleResources { sequence } => Self::SampleResources { sequence },
            WireEffect::ListChildren { parent } => Self::ListChildren {
                parent: parent.try_into()?,
            },
            WireEffect::AcquireChildIdentity { discovery } => Self::AcquireChildIdentity {
                discovery: discovery.try_into()?,
            },
            WireEffect::ObserveIdentity { identity } => Self::ObserveIdentity {
                identity: identity.try_into()?,
            },
            WireEffect::SignalExactIdentity { identity, signal } => Self::SignalExactIdentity {
                identity: identity.try_into()?,
                signal: signal.into(),
            },
            WireEffect::ReverifyRetainedCandidate { commit, digest } => {
                Self::ReverifyRetainedCandidate {
                    candidate: RetainedCandidateIdentity {
                        source_commit: FullCommitSha::from_bytes(commit),
                        artifact_digest: ArtifactDigest::from_bytes(digest),
                    },
                }
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireState {
    Active,
    DetachedFenced,
    Reconciling,
    AuthorityLost,
}
impl From<GuardianState> for WireState {
    fn from(value: GuardianState) -> Self {
        match value {
            GuardianState::Active => Self::Active,
            GuardianState::DetachedFenced => Self::DetachedFenced,
            GuardianState::Reconciling => Self::Reconciling,
            GuardianState::AuthorityLost => Self::AuthorityLost,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireObservation {
    Running,
    Exited,
    Unavailable,
}
impl From<ChildSlotObservation> for WireObservation {
    fn from(value: ChildSlotObservation) -> Self {
        match value {
            ChildSlotObservation::Running => Self::Running,
            ChildSlotObservation::Exited => Self::Exited,
            ChildSlotObservation::Unavailable => Self::Unavailable,
        }
    }
}
/// Diagnostic metadata only; never a session or dispatch capability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireNativeBinding {
    pub attempt: u128,
    pub deadline: u64,
    pub nonce: [u8; 16],
    pub guardian: WireIdentity,
    pub slots: Vec<WireNativeSlot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireNativeSlot {
    pub identity: WireIdentity,
    pub liveness: WireObservation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSnapshot {
    pub instance: u64,
    pub attempt: u128,
    pub deadline: u64,
    pub state: WireState,
    pub session: WireSession,
    pub slots: Vec<WireSlot>,
    pub retained: Vec<WireRetained>,
    pub native_binding: Option<WireNativeBinding>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireRetained {
    pub key: WireKey,
    pub completion: WireCompletion,
}
// Completions are observations, not deserialized ExecutedEffect values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireCompletion {
    SampleResources {
        sequence: u64,
        result: Result<WireResourceSample, WireResourceError>,
    },
    ListChildren {
        parent: WireIdentity,
        result: Result<Vec<u32>, WireQueryError>,
    },
    AcquireChildIdentity {
        discovery: WireDiscovery,
        observation: WireChildIdentityObservation,
    },
    ObserveIdentity {
        identity: WireIdentity,
        observation: WireIdentityObservation,
    },
    SignalExactIdentity {
        identity: WireIdentity,
        signal: WireSignal,
        outcome: WireSignalOutcome,
    },
    ReverifyRetainedCandidate {
        commit: [u8; 20],
        digest: [u8; 32],
        verification: WireCandidateVerification,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireResourceSample {
    pub sequence: u64,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub cpu_basis_points: u16,
    pub memory_pressure: WireMemoryPressure,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireMemoryPressure {
    Normal,
    Elevated,
    Critical,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireResourceError {
    TemporarilyUnavailable,
    Unsupported,
    Malformed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireQueryError {
    PermissionDenied,
    TemporarilyUnavailable,
    Malformed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireChildIdentityObservation {
    Stable { identity: WireIdentity },
    Unresolved(WireObservationFailure),
    DifferentProcess(WireIdentity),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WireIdentityObservation {
    SameIdentity {
        current_parent: Option<WireIdentity>,
    },
    ExactIdentityAbsent,
    PidReused(WireIdentity),
    Unresolved(WireObservationFailure),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireObservationFailure {
    NoSuchProcess,
    PermissionDenied,
    TemporarilyUnavailable,
    Malformed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSignalOutcome {
    Delivered,
    ExactIdentityAlreadyAbsent,
    PidReused,
    PermissionDenied,
    TemporarilyUnavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireCandidateVerification {
    Exact,
    Missing,
    Mismatch,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    Hello {
        build: BuildIdentity,
    },
    Snapshot,
    Fence {
        session: WireSession,
    },
    BeginReconciliation {
        instance: u64,
        inventory: Vec<WireSlot>,
        interruption_revision: u64,
    },
    FinishReconciliation {
        session: WireSession,
        applied_revision: u64,
        now: u64,
    },
    ActivateReconciliation {
        session: WireSession,
        attempt: u128,
        recovery_revision: u64,
        binding_revision: u64,
    },
    ReleaseInitial {
        session: WireSession,
        attempt: u128,
        nonce: [u8; 16],
        attempt_revision: u64,
    },
    Inventory,
    ObserveSlot {
        slot: WireSlot,
    },
    ObserveRetained {
        key: WireKey,
    },
    VerifyRecoveredNativeSampling {
        session: WireSession,
        key: WireKey,
        sequence: u64,
        issuing_session_generation: u64,
    },
    AckAfterAppliedRevision {
        session: WireSession,
        key: WireKey,
        applied_revision: u64,
    },
    ReserveSignalRound {
        session: WireSession,
        revision: u64,
        count: u32,
    },
    DispatchAdmit {
        session: WireSession,
        key: WireKey,
        effect: WireEffect,
    },
    DispatchSubmit {
        session: WireSession,
        admission: u64,
        now: u64,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Response {
    Hello {
        build: BuildIdentity,
        snapshot: WireSnapshot,
    },
    Snapshot(WireSnapshot),
    Fenced,
    ReconciliationBegun {
        session: WireSession,
    },
    InitialReleased {
        session: WireSession,
    },
    ReconciliationPrepared {
        session: WireSession,
        attempt: u128,
        recovery_revision: u64,
    },
    ReconciliationFinished,
    Inventory(Vec<WireSlot>),
    SlotObserved(WireObservation),
    Retained(Option<WireRetained>),
    RecoveredNativeSampling {
        key: WireKey,
        sequence: u64,
        sample: WireResourceSample,
    },
    Acked {
        key: WireKey,
        applied_revision: u64,
    },
    SignalRoundReserved,
    Admitted {
        admission: u64,
    },
    Submitted {
        key: WireKey,
    },
    Refused(WireError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireError {
    InvalidValue,
    BuildMismatch,
    UnknownAdmission,
    AdmissionReplayed,
    AdmissionMismatch,
    NoStagedTicket,
    MissingProof,
    UnsupportedDurableBinding,
    Guardian(WireGuardianError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireGuardianError {
    InstanceIdExhausted,
    SessionGenerationExhausted,
    ChildSlotCapacityExceeded,
    RetainedResultCapacityExceeded,
    StaleSession,
    StaleDispatchRevision,
    NotActive,
    NotDetached,
    NotReconciling,
    SessionFenced,
    AttemptMismatch,
    InstanceMismatch,
    InventoryMismatch,
    PermitMismatch,
    AckMismatch,
    DuplicateDispatchKey,
    ResultNotRetained,
    UnknownChildSlot,
    DeadlineExpired,
    AuthorityLost,
    ExecutorCorrelationMismatch,
}
impl From<GuardianError> for WireError {
    fn from(value: GuardianError) -> Self {
        use GuardianError as G;
        Self::Guardian(match value {
            G::InstanceIdExhausted => WireGuardianError::InstanceIdExhausted,
            G::SessionGenerationExhausted => WireGuardianError::SessionGenerationExhausted,
            G::ChildSlotCapacityExceeded => WireGuardianError::ChildSlotCapacityExceeded,
            G::RetainedResultCapacityExceeded => WireGuardianError::RetainedResultCapacityExceeded,
            G::StaleSession => WireGuardianError::StaleSession,
            G::StaleDispatchRevision => WireGuardianError::StaleDispatchRevision,
            G::NotActive => WireGuardianError::NotActive,
            G::NotDetached => WireGuardianError::NotDetached,
            G::NotReconciling => WireGuardianError::NotReconciling,
            G::SessionFenced => WireGuardianError::SessionFenced,
            G::AttemptMismatch => WireGuardianError::AttemptMismatch,
            G::InstanceMismatch => WireGuardianError::InstanceMismatch,
            G::InventoryMismatch => WireGuardianError::InventoryMismatch,
            G::PermitMismatch => WireGuardianError::PermitMismatch,
            G::AckMismatch => WireGuardianError::AckMismatch,
            G::DuplicateDispatchKey => WireGuardianError::DuplicateDispatchKey,
            G::ResultNotRetained => WireGuardianError::ResultNotRetained,
            G::UnknownChildSlot => WireGuardianError::UnknownChildSlot,
            G::DeadlineExpired => WireGuardianError::DeadlineExpired,
            G::AuthorityLost => WireGuardianError::AuthorityLost,
            G::ExecutorCorrelationMismatch => WireGuardianError::ExecutorCorrelationMismatch,
        })
    }
}

#[derive(Debug)]
pub(crate) struct NativeSamplingAdmission {
    session: ControllerSession,
    key: DispatchKey,
    sequence: u64,
}

impl NativeSamplingAdmission {
    const fn key(&self) -> DispatchKey {
        self.key
    }

    const fn session(&self) -> ControllerSession {
        self.session
    }

    const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn into_ticket(self) -> (ControllerSession, DispatchTicket) {
        let Self {
            session,
            key,
            sequence,
        } = self;
        (
            session,
            DispatchTicket::from_committed_parts(key, OsEffect::SampleResources { sequence }),
        )
    }
}

fn recovered_sampling_slots_match(
    binding: &crate::NativeGuardianBinding,
    wire: &WireNativeBinding,
    live: &WireSnapshot,
    diagnostics: &[(Option<ProcessIdentity>, ChildSlotObservation)],
) -> bool {
    let slots = binding.slots();
    slots.len() == wire.slots.len()
        && slots.len() == live.slots.len()
        && slots.len() == diagnostics.len()
        && slots
            .iter()
            .zip(&wire.slots)
            .zip(&live.slots)
            .zip(diagnostics)
            .enumerate()
            .all(|(index, (((stored, native), slot), diagnostic))| {
                let observation = match stored.liveness {
                    crate::NativeSlotLiveness::Running => WireObservation::Running,
                    crate::NativeSlotLiveness::Exited => WireObservation::Exited,
                    crate::NativeSlotLiveness::Unavailable => WireObservation::Unavailable,
                };
                let observed_slot = match stored.liveness {
                    crate::NativeSlotLiveness::Running => ChildSlotObservation::Running,
                    crate::NativeSlotLiveness::Exited => ChildSlotObservation::Exited,
                    crate::NativeSlotLiveness::Unavailable => ChildSlotObservation::Unavailable,
                };
                WireIdentity::from(stored.process) == native.identity
                    && native.liveness == observation
                    && slot.instance == live.instance
                    && slot.ordinal == (index + 1) as u64
                    && *diagnostic == (Some(stored.process), observed_slot)
            })
}

// Affine authority is held server-side; only correlation keys can be decoded.
pub struct GuardianServer<E: GuardianExecutor> {
    guardian: Guardian<E>,
    session: ControllerSession,
    build: BuildIdentity,
    hello_verified: bool,
    interruption: Option<PersistenceRevision>,
    finish_proof: Option<(PersistenceRevision, crate::ReconciledSessionPermit)>,
    prepared_activation: Option<(
        crate::NativeReconciliationCompletion,
        crate::guardian::PreparedReconciliationActivation,
    )>,
    ack_proofs: Vec<(DispatchKey, PersistenceRevision, crate::AppliedResultAck)>,
    native_ack_receipts: Vec<(
        crate::NativeSamplingIssue,
        crate::ResourceSample,
        PersistenceRevision,
    )>,
    staged: Vec<DispatchTicket>,
    refused: Vec<DispatchTicket>,
    admissions: BTreeMap<u64, (ControllerSession, DispatchTicket)>,
    native_admissions: BTreeMap<u64, NativeSamplingAdmission>,
    native_retained_origins: Vec<(DispatchKey, u64, NonZeroU64)>,
    next_admission: u64,
    native_unbound: bool,
    native_identity: Option<crate::NativeGuardianIdentity>,
    native_reader: Option<Box<dyn NativeEvidenceReader>>,
    initial_gate: Option<Box<dyn NativeInitialGate>>,
}
impl<E: GuardianExecutor> GuardianServer<E> {
    /// Construct a server bound to an authenticated policy session.
    ///
    /// # Errors
    /// Returns the guardian's authentication error if the supplied session
    /// does not match its instance, attempt, deadline, or active authority.
    pub fn new(
        guardian: Guardian<E>,
        session: ControllerSession,
        build: BuildIdentity,
    ) -> Result<Self, GuardianError> {
        guardian.authenticate_policy_binding(
            session,
            guardian.attempt_id(),
            guardian.original_deadline(),
        )?;
        Ok(Self {
            guardian,
            session,
            build,
            hello_verified: false,
            interruption: None,
            finish_proof: None,
            prepared_activation: None,
            ack_proofs: Vec::new(),
            native_ack_receipts: Vec::new(),
            staged: Vec::new(),
            refused: Vec::new(),
            admissions: BTreeMap::new(),
            native_admissions: BTreeMap::new(),
            native_retained_origins: Vec::new(),
            next_admission: 1,
            native_unbound: false,
            native_identity: None,
            native_reader: None,
            initial_gate: None,
        })
    }
    /// Disable native mutation until durable policy binding can stage affine proof.
    /// The disposable process may expose observations, but cannot fabricate a
    /// committed transition, interruption, reconciliation or applied ACK.
    pub fn require_native_binding(&mut self) {
        self.native_unbound = true;
    }

    /// Stage this process's diagnostic identity; mutation remains unsupported.
    pub fn stage_native_identity(&mut self, identity: crate::NativeGuardianIdentity) {
        self.native_identity = Some(identity);
        self.require_native_binding();
    }

    /// Pins the trusted native evidence reader for this guardian lifetime.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn stage_native_reader(&mut self, reader: Box<dyn NativeEvidenceReader>) {
        self.native_reader = Some(reader);
    }

    pub fn stage_initial_gate(&mut self, gate: Box<dyn NativeInitialGate>) {
        self.initial_gate = Some(gate);
    }

    fn release_initial(
        &mut self,
        session: WireSession,
        attempt: u128,
        nonce: [u8; 16],
        attempt_revision: u64,
    ) -> Result<Response, WireError> {
        if !self.native_unbound || self.native_reader.is_none() || self.initial_gate.is_none() {
            return Err(WireError::UnsupportedDurableBinding);
        }
        self.resolve_session(session)?;
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        let (checkpoint, binding) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .read(self.guardian.attempt_id())
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let revision = revision(attempt_revision)?;
        let current = binding.current_session();
        let gate = self
            .initial_gate
            .as_ref()
            .ok_or(WireError::UnsupportedDurableBinding)?;
        if live.state != WireState::Active
            || session != live.session
            || session.generation != 1
            || attempt != live.attempt
            || attempt != wire.attempt
            || nonce != wire.nonce
            || binding.attempt_id() != self.guardian.attempt_id()
            || checkpoint.attempt_id() != self.guardian.attempt_id()
            || checkpoint.phase() != crate::Phase::ResourceSampling
            || binding.identity().nonce.bytes() != wire.nonce
            || WireIdentity::from(binding.identity().process) != wire.guardian
            || self.native_identity != Some(binding.identity())
            || binding.deadline().get() != wire.deadline
            || wire.deadline != live.deadline
            || !before_native_deadline(wire.deadline)
            || checkpoint.persistence_revision() != revision
            || binding.dispatch_floor() != revision
            || binding.revision() != PersistenceRevision::first()
            || binding.sessions().len() != 1
            || current.interruption_revision != revision
            || current.generation.get() != session.generation
            || current.state != crate::NativeSessionState::Active
            || binding.slots().len() != 1
            || wire.slots.len() != 1
            || live.slots.len() != 1
        {
            return Err(WireError::MissingProof);
        }
        if WireIdentity::from(binding.slots()[0].process) != wire.slots[0].identity
            || binding.slots()[0].process != gate.identity()
            || binding.slots()[0].liveness != crate::NativeSlotLiveness::Running
            || wire.slots[0].liveness != WireObservation::Running
            || live.slots[0].instance != live.instance
            || live.slots[0].ordinal != 1
            || self
                .guardian
                .diagnostic_slots()
                .first()
                .is_none_or(|(identity, liveness)| {
                    *identity != Some(gate.identity()) || *liveness != ChildSlotObservation::Running
                })
        {
            return Err(WireError::MissingProof);
        }
        // Consume before writing: even ambiguous pipe errors cannot authorize a retry.
        let gate = self
            .initial_gate
            .take()
            .ok_or(WireError::UnsupportedDurableBinding)?;
        gate.release().map_err(|_| WireError::MissingProof)?;
        Ok(Response::InitialReleased { session })
    }

    /// Returns the live guardian without exposing mutable custody.
    pub fn guardian(&self) -> &Guardian<E> {
        &self.guardian
    }

    /// Stages the exact interruption revision produced by the trusted durable runtime.
    pub fn stage_interruption(&mut self, revision: PersistenceRevision) {
        self.interruption = Some(revision);
    }

    /// Stages a reconciliation proof minted by the trusted durable runtime.
    pub fn stage_finish_proof(
        &mut self,
        revision: PersistenceRevision,
        proof: crate::ReconciledSessionPermit,
    ) {
        self.finish_proof = Some((revision, proof));
    }
    /// Stages an acknowledgement proof minted only after durable result application.
    pub fn stage_applied_ack(
        &mut self,
        key: DispatchKey,
        revision: PersistenceRevision,
        ack: crate::AppliedResultAck,
    ) {
        self.ack_proofs.push((key, revision, ack));
    }
    /// Returns definite pre-execution refusals to the trusted affine runtime.
    pub fn take_refused(&mut self) -> Vec<DispatchTicket> {
        std::mem::take(&mut self.refused)
    }

    /// Stages an affine ticket issued from a committed policy transition.
    ///
    /// # Errors
    /// Returns the unchanged ticket when its attempt does not match or its key is
    /// already staged, refused, or admitted.
    pub fn stage_committed(&mut self, ticket: DispatchTicket) -> Result<(), DispatchTicket> {
        if ticket.key().attempt_id() != self.guardian.attempt_id()
            || self
                .staged
                .iter()
                .any(|existing| existing.key() == ticket.key())
            || self
                .refused
                .iter()
                .any(|existing| existing.key() == ticket.key())
            || self
                .admissions
                .values()
                .any(|(_, existing)| existing.key() == ticket.key())
        {
            return Err(ticket);
        }
        self.staged.push(ticket);
        Ok(())
    }
    fn resolve_session(&self, value: WireSession) -> Result<ControllerSession, WireError> {
        if value.instance != self.guardian.instance_id().get() {
            return Err(GuardianError::InstanceMismatch.into());
        }
        if value != self.session.into() {
            return Err(GuardianError::StaleSession.into());
        }
        Ok(self.session)
    }
    fn resolve_key(&self, value: WireKey) -> Result<DispatchKey, WireError> {
        if value.attempt != self.guardian.attempt_id().get() {
            return Err(GuardianError::AttemptMismatch.into());
        }
        Ok(DispatchKey::from_committed_parts(
            self.guardian.attempt_id(),
            revision(value.revision)?,
            value
                .ordinal
                .try_into()
                .map_err(|_| WireError::InvalidValue)?,
        ))
    }
    fn slot(&self, value: WireSlot) -> Result<crate::ChildSlotId, WireError> {
        if value.instance != self.guardian.instance_id().get() {
            return Err(GuardianError::InstanceMismatch.into());
        }
        self.guardian
            .inventory()
            .find(|slot| WireSlot::from(*slot) == value)
            .ok_or(GuardianError::UnknownChildSlot.into())
    }
    fn snapshot(&self) -> WireSnapshot {
        WireSnapshot {
            instance: self.guardian.instance_id().get(),
            attempt: self.guardian.attempt_id().get(),
            deadline: self.guardian.original_deadline().get(),
            state: self.guardian.state().into(),
            session: self.session.into(),
            slots: self.guardian.inventory().map(Into::into).collect(),
            retained: self
                .guardian
                .retained_results()
                .map(WireRetained::from)
                .collect(),
            native_binding: self.native_identity.and_then(|identity| {
                self.guardian
                    .diagnostic_slots()
                    .into_iter()
                    .map(|(process, liveness)| {
                        Some(WireNativeSlot {
                            identity: process?.into(),
                            liveness: liveness.into(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(|slots| WireNativeBinding {
                        attempt: self.guardian.attempt_id().get(),
                        deadline: self.guardian.original_deadline().get(),
                        nonce: identity.nonce.bytes(),
                        guardian: identity.process.into(),
                        slots,
                    })
            }),
        }
    }
    pub fn fence_connection(&mut self) {
        self.hello_verified = false;
        let _ = self.guardian.controller_lost(self.session);
        self.initial_gate.take();
        self.staged.clear();
        self.refused.clear();
        self.admissions.clear();
        self.native_admissions.clear();
        self.interruption = None;
        self.finish_proof = None;
        self.prepared_activation = None;
        self.ack_proofs.clear();
    }
    pub fn handle(&mut self, request: Request) -> Response {
        if let Request::Hello { build } = &request {
            if build != &self.build {
                self.fence_connection();
                return Response::Refused(WireError::BuildMismatch);
            }
            self.hello_verified = true;
        } else if !self.hello_verified {
            return Response::Refused(WireError::BuildMismatch);
        }
        if self.native_unbound
            && !matches!(
                request,
                Request::Hello { .. }
                    | Request::Snapshot
                    | Request::Inventory
                    | Request::ObserveSlot { .. }
                    | Request::ObserveRetained { .. }
                    | Request::VerifyRecoveredNativeSampling { .. }
                    | Request::Fence { .. }
                    | Request::BeginReconciliation { .. }
                    | Request::FinishReconciliation { .. }
                    | Request::ActivateReconciliation { .. }
                    | Request::ReleaseInitial { .. }
                    | Request::DispatchAdmit { .. }
                    | Request::DispatchSubmit { .. }
                    | Request::AckAfterAppliedRevision { .. }
            )
        {
            return Response::Refused(WireError::UnsupportedDurableBinding);
        }
        match self.try_handle(request) {
            Ok(response) => response,
            Err(error) => Response::Refused(error),
        }
    }
    fn begin_reconciliation(
        &mut self,
        instance: u64,
        inventory: Vec<WireSlot>,
        interruption_revision: u64,
    ) -> Result<Response, WireError> {
        Ok({
            if instance != self.guardian.instance_id().get() {
                return Err(GuardianError::InstanceMismatch.into());
            }
            if self.native_unbound {
                if self.native_reader.is_none() {
                    return Err(WireError::UnsupportedDurableBinding);
                }
                let revision = revision(interruption_revision)?;
                let live = self.snapshot();
                let wire = live
                    .native_binding
                    .as_ref()
                    .ok_or(WireError::MissingProof)?;
                let reader = self
                    .native_reader
                    .as_mut()
                    .ok_or(WireError::UnsupportedDurableBinding)?;
                let (checkpoint, binding) = reader
                    .read(self.guardian.attempt_id())
                    .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
                    .ok_or(WireError::MissingProof)?;
                let next = self
                    .session
                    .generation()
                    .checked_add(1)
                    .ok_or(WireError::InvalidValue)?;
                let current = binding.current_session();
                if checkpoint.phase() != crate::Phase::Interrupted
                    || checkpoint.persistence_revision() != revision
                    || binding.attempt_id() != self.guardian.attempt_id()
                    || binding.identity().nonce.bytes() != wire.nonce
                    || WireIdentity::from(binding.identity().process) != wire.guardian
                    || binding.deadline().get() != self.guardian.original_deadline().get()
                    || wire.deadline != binding.deadline().get()
                    || wire.attempt != self.guardian.attempt_id().get()
                    || binding.dispatch_floor() != revision
                    || current.generation.get() != next
                    || current.state != crate::NativeSessionState::Reconciling
                    || current.interruption_revision != revision
                    || live.state != WireState::DetachedFenced
                    || inventory != live.slots
                    || binding.slots().len() != wire.slots.len()
                    || binding
                        .slots()
                        .iter()
                        .zip(&wire.slots)
                        .any(|(stored, live)| WireIdentity::from(stored.process) != live.identity)
                {
                    return Err(WireError::MissingProof);
                }
                if !before_native_deadline(wire.deadline) {
                    return Err(WireError::MissingProof);
                }
                self.interruption = Some(revision);
            }
            let slots = inventory
                .into_iter()
                .map(|slot| self.slot(slot))
                .collect::<Result<Vec<_>, _>>()?;
            let revision = revision(interruption_revision)?;
            if self.interruption != Some(revision) {
                return Err(WireError::MissingProof);
            }
            let session = self
                .guardian
                .begin_reconciliation(self.guardian.instance_id(), &slots, revision)
                .map_err(WireError::from)?;
            self.interruption = None;
            self.session = session;
            self.staged.clear();
            self.refused.clear();
            self.admissions.clear();
            self.native_admissions.clear();
            self.finish_proof = None;
            self.prepared_activation = None;
            self.ack_proofs.clear();
            Response::ReconciliationBegun {
                session: session.into(),
            }
        })
    }
    fn finish_reconciliation(
        &mut self,
        session: WireSession,
        applied_revision: u64,
        now: u64,
    ) -> Result<Response, WireError> {
        Ok({
            let session = self.resolve_session(session)?;
            let revision = revision(applied_revision)?;
            if self.native_unbound {
                return self.finish_native_reconciliation(session, revision);
            }
            if self.finish_proof.as_ref().map(|(stored, _)| *stored) != Some(revision) {
                return Err(WireError::MissingProof);
            }
            let (_, permit) = self.finish_proof.take().ok_or(WireError::MissingProof)?;
            self.guardian
                .finish_reconciliation(session, permit, MonotonicTick::new(now))
                .map_err(WireError::from)?;
            Response::ReconciliationFinished
        })
    }
    fn finish_native_reconciliation(
        &mut self,
        session: ControllerSession,
        revision: PersistenceRevision,
    ) -> Result<Response, WireError> {
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        if live.state != WireState::Reconciling || self.prepared_activation.is_some() {
            return Err(WireError::MissingProof);
        }
        let generation = NonZeroU64::new(session.generation()).ok_or(WireError::InvalidValue)?;
        let (checkpoint, binding, completion) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .reconciliation_evidence(self.guardian.attempt_id(), generation)
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let current = binding.current_session();
        if checkpoint.attempt_id() != self.guardian.attempt_id()
            || checkpoint.phase() != crate::Phase::ResourceSampling
            || checkpoint.persistence_revision() != revision
            || checkpoint.deadline() != self.guardian.original_deadline()
            || completion.attempt_id() != self.guardian.attempt_id()
            || completion.session_generation() != generation
            || completion.recovery_revision() != revision
            || completion.interruption_revision().checked_next() != Some(revision)
            || completion.binding_revision() != binding.revision()
            || binding.attempt_id() != self.guardian.attempt_id()
            || binding.identity().nonce.bytes() != wire.nonce
            || WireIdentity::from(binding.identity().process) != wire.guardian
            || binding.deadline() != self.guardian.original_deadline()
            || wire.deadline != binding.deadline().get()
            || wire.attempt != self.guardian.attempt_id().get()
            || current.generation != generation
            || current.state != crate::NativeSessionState::Reconciling
            || current.interruption_revision != completion.interruption_revision()
            || self.guardian.reconciliation_floor() != Some(completion.interruption_revision())
            || binding.dispatch_floor() != completion.interruption_revision()
            || binding.slots().len() != wire.slots.len()
            || binding
                .slots()
                .iter()
                .zip(&wire.slots)
                .any(|(stored, live)| WireIdentity::from(stored.process) != live.identity)
        {
            return Err(WireError::MissingProof);
        }
        // Native time, not the wire's claimed `now`, owns deadline admission.
        let now = native_reconciliation_now()?;
        if now >= self.guardian.original_deadline() {
            return Err(WireError::MissingProof);
        }
        let permit = crate::ReconciledSessionPermit::after_committed_reconciliation(
            session,
            self.guardian.attempt_id(),
            binding.deadline(),
            revision,
        );
        let prepared = self
            .guardian
            .prepare_reconciliation(session, permit, now)
            .map_err(WireError::from)?;
        self.prepared_activation = Some((completion, prepared));
        Ok(Response::ReconciliationPrepared {
            session: session.into(),
            attempt: self.guardian.attempt_id().get(),
            recovery_revision: revision.get(),
        })
    }

    fn activate_native_reconciliation(
        &mut self,
        session: WireSession,
        attempt: u128,
        recovery_revision: u64,
        binding_revision: u64,
    ) -> Result<Response, WireError> {
        if !self.native_unbound {
            return Err(WireError::UnsupportedDurableBinding);
        }
        let session = self.resolve_session(session)?;
        let generation = NonZeroU64::new(session.generation()).ok_or(WireError::InvalidValue)?;
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        let staged = self
            .prepared_activation
            .as_ref()
            .ok_or(WireError::MissingProof)?
            .0;
        if live.state != WireState::Reconciling
            || attempt != self.guardian.attempt_id().get()
            || recovery_revision != staged.recovery_revision().get()
            || staged.session_generation() != generation
        {
            return Err(WireError::MissingProof);
        }
        let (checkpoint, binding, activation) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .activation_evidence(self.guardian.attempt_id(), generation)
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let current = binding.current_session();
        if activation.completion() != staged
            || activation.binding_revision().get() != binding_revision
            || staged.binding_revision().checked_next() != Some(activation.binding_revision())
            || binding.revision() != activation.binding_revision()
            || binding.dispatch_floor() != staged.recovery_revision()
            || checkpoint.attempt_id() != self.guardian.attempt_id()
            || checkpoint.phase() != crate::Phase::ResourceSampling
            || checkpoint.persistence_revision() != staged.recovery_revision()
            || checkpoint.deadline() != self.guardian.original_deadline()
            || binding.attempt_id() != self.guardian.attempt_id()
            || binding.identity().nonce.bytes() != wire.nonce
            || WireIdentity::from(binding.identity().process) != wire.guardian
            || binding.deadline() != self.guardian.original_deadline()
            || wire.deadline != binding.deadline().get()
            || wire.attempt != attempt
            || current.generation != generation
            || current.interruption_revision != staged.interruption_revision()
            || current.state != crate::NativeSessionState::Active
            || self.guardian.reconciliation_floor() != Some(staged.interruption_revision())
            || binding.slots().len() != wire.slots.len()
            || binding
                .slots()
                .iter()
                .zip(&wire.slots)
                .any(|(stored, live)| WireIdentity::from(stored.process) != live.identity)
        {
            return Err(WireError::MissingProof);
        }
        let (_, prepared) = self
            .prepared_activation
            .take()
            .ok_or(WireError::MissingProof)?;
        self.guardian
            .activate_reconciliation(session, prepared, native_reconciliation_now()?)
            .map_err(WireError::from)?;
        Ok(Response::ReconciliationFinished)
    }
    fn ack_after_applied_revision(
        &mut self,
        session: WireSession,
        key: WireKey,
        applied_revision: u64,
    ) -> Result<Response, WireError> {
        Ok({
            let session = self.resolve_session(session)?;
            let key = self.resolve_key(key)?;
            let applied_revision = revision(applied_revision)?;
            if self.native_unbound {
                return self.native_sampling_ack(session, key, applied_revision);
            }
            if applied_revision < key.revision() {
                return Err(GuardianError::StaleDispatchRevision.into());
            }
            let position = self
                .ack_proofs
                .iter()
                .position(|(stored_key, stored_revision, _)| {
                    *stored_key == key && *stored_revision == applied_revision
                })
                .ok_or(WireError::MissingProof)?;
            let (_, _, ack) = self.ack_proofs.swap_remove(position);
            self.guardian
                .ack_result(session, ack)
                .map_err(WireError::from)?;
            Response::Acked {
                key: key.try_into()?,
                applied_revision: applied_revision.get(),
            }
        })
    }
    fn verify_recovered_native_sampling(
        &mut self,
        session: WireSession,
        key: WireKey,
        sequence: u64,
        issuing_session_generation: u64,
    ) -> Result<Response, WireError> {
        if !self.native_unbound {
            return Err(WireError::UnsupportedDurableBinding);
        }
        self.resolve_session(session)?;
        let key = self.resolve_key(key)?;
        let origin = NonZeroU64::new(issuing_session_generation).ok_or(WireError::InvalidValue)?;
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        let (checkpoint, binding, status) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .sampling_evidence(key)
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let crate::NativeSamplingStatus::IssuedPendingApplication { issue } = status else {
            return Err(WireError::MissingProof);
        };
        let current = binding.current_session();
        if live.state != WireState::Reconciling
            || live.session != session
            || live.session.instance != live.instance
            || live.session.generation == 0
            || live.attempt != key.attempt_id().get()
            || wire.attempt != live.attempt
            || wire.deadline != live.deadline
            || binding.attempt_id() != key.attempt_id()
            || checkpoint.attempt_id() != key.attempt_id()
            || checkpoint.phase() != crate::Phase::Interrupted
            || checkpoint.interrupted_sampling_sequence() != Some(sequence)
            || checkpoint.deadline() != binding.deadline()
            || checkpoint.persistence_revision() != current.interruption_revision
            || binding.dispatch_floor() != current.interruption_revision
            || key.revision() >= current.interruption_revision
            || current.state != crate::NativeSessionState::Reconciling
            || current.generation.get() != session.generation
            || origin >= current.generation
            || !binding
                .sessions()
                .iter()
                .any(|old| old.generation == origin)
            || issue.key() != key
            || issue.sequence() != sequence
            || issue.session_generation() != origin
            || issue.guardian_identity() != binding.identity()
            || !self
                .native_retained_origins
                .contains(&(key, sequence, origin))
            || self.native_identity != Some(binding.identity())
            || binding.deadline() != self.guardian.original_deadline()
            || wire.nonce != binding.identity().nonce.bytes()
            || wire.guardian != WireIdentity::from(binding.identity().process)
            || !recovered_sampling_slots_match(
                &binding,
                wire,
                &live,
                &self.guardian.diagnostic_slots(),
            )
            || !before_native_deadline(live.deadline)
        {
            return Err(WireError::MissingProof);
        }
        let Some(retained) = self.guardian.retained_result(key) else {
            return Err(WireError::MissingProof);
        };
        let WireCompletion::SampleResources {
            sequence: returned,
            result: Ok(sample),
        } = WireRetained::from(retained).completion
        else {
            return Err(WireError::MissingProof);
        };
        if returned != sequence
            || sample.sequence != sequence
            || crate::CpuContentionBasisPoints::new(sample.cpu_basis_points).is_err()
        {
            return Err(WireError::MissingProof);
        }
        Ok(Response::RecoveredNativeSampling {
            key: key.try_into()?,
            sequence,
            sample,
        })
    }

    fn native_sampling_ack(
        &mut self,
        session: ControllerSession,
        key: DispatchKey,
        applied_revision: PersistenceRevision,
    ) -> Result<Response, WireError> {
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        let (checkpoint, binding, status) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .sampling_evidence(key)
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let crate::NativeSamplingStatus::AppliedPendingAck {
            issue,
            sample,
            applying_revision,
        } = status
        else {
            return Err(WireError::MissingProof);
        };
        let current = binding.current_session();
        if applying_revision != applied_revision
            || issue.key() != key
            || issue.sequence() != sample.sequence
            || issue.guardian_identity() != binding.identity()
            || !binding
                .sessions()
                .iter()
                .any(|s| s.generation == issue.session_generation())
            || self.native_identity != Some(binding.identity())
            || wire.guardian != WireIdentity::from(binding.identity().process)
            || wire.nonce != binding.identity().nonce.bytes()
            || wire.attempt != live.attempt
            || wire.deadline != live.deadline
            || binding.attempt_id() != self.guardian.attempt_id()
            || binding.deadline() != self.guardian.original_deadline()
            || checkpoint.attempt_id() != self.guardian.attempt_id()
            || checkpoint.deadline() != self.guardian.original_deadline()
            || checkpoint.persistence_revision() < applying_revision
            || current.generation.get() != session.generation()
            || live.session != session.into()
            || !matches!(
                (live.state, current.state),
                (WireState::Active, crate::NativeSessionState::Active)
                    | (
                        WireState::Reconciling,
                        crate::NativeSessionState::Reconciling
                    )
            )
            || !before_native_deadline(live.deadline)
        {
            return Err(WireError::MissingProof);
        }
        let receipt = (issue, sample, applying_revision);
        if self.native_ack_receipts.contains(&receipt) {
            if self.guardian.retained_result(key).is_some() {
                return Err(WireError::MissingProof);
            }
        } else {
            let expected = WireCompletion::SampleResources {
                sequence: issue.sequence(),
                result: Ok(WireResourceSample {
                    sequence: sample.sequence,
                    memory_bytes: sample.capacity.available_memory_bytes,
                    storage_bytes: sample.capacity.available_storage_bytes,
                    cpu_basis_points: sample.contention.cpu.get(),
                    memory_pressure: match sample.contention.memory_pressure {
                        crate::MemoryPressure::Nominal => WireMemoryPressure::Normal,
                        crate::MemoryPressure::Warning => WireMemoryPressure::Elevated,
                        crate::MemoryPressure::Critical => WireMemoryPressure::Critical,
                    },
                }),
            };
            if self
                .guardian
                .retained_result(key)
                .is_none_or(|result| WireRetained::from(result).completion != expected)
            {
                return Err(WireError::MissingProof);
            }
            self.guardian
                .ack_result(
                    session,
                    crate::AppliedResultAck::after_applied(session, key),
                )
                .map_err(WireError::from)?;
            self.native_retained_origins
                .retain(|(stored, _, _)| *stored != key);
            self.native_ack_receipts.push(receipt);
        }
        Ok(Response::Acked {
            key: key.try_into()?,
            applied_revision: applied_revision.get(),
        })
    }

    fn validate_native_sampling_evidence(
        &mut self,
        session: ControllerSession,
        key: DispatchKey,
        sequence: u64,
    ) -> Result<(), WireError> {
        let live = self.snapshot();
        let wire = live
            .native_binding
            .as_ref()
            .ok_or(WireError::MissingProof)?;
        let (checkpoint, binding, status) = self
            .native_reader
            .as_mut()
            .ok_or(WireError::UnsupportedDurableBinding)?
            .sampling_evidence(key)
            .map_err(|NativeEvidenceReadError| WireError::MissingProof)?
            .ok_or(WireError::MissingProof)?;
        let crate::NativeSamplingStatus::IssuedPendingApplication { issue } = status else {
            return Err(WireError::MissingProof);
        };
        if live.state != WireState::Active
            || live.session != session.into()
            || wire.attempt != live.attempt
            || wire.deadline != live.deadline
            || wire.nonce != binding.identity().nonce.bytes()
            || wire.guardian != WireIdentity::from(binding.identity().process)
            || self.native_identity != Some(binding.identity())
            || binding.attempt_id() != self.guardian.attempt_id()
            || binding.deadline() != self.guardian.original_deadline()
            || binding.current_session().state != crate::NativeSessionState::Active
            || binding.current_session().generation.get() != session.generation()
            || checkpoint.attempt_id() != self.guardian.attempt_id()
            || checkpoint.phase() != crate::Phase::ResourceSampling
            || checkpoint.persistence_revision() != key.revision()
            || binding.dispatch_floor() > key.revision()
            || issue.key() != key
            || issue.sequence() != sequence
            || issue.session_generation().get() != session.generation()
            || issue.guardian_identity() != binding.identity()
            || !before_native_deadline(live.deadline)
        {
            return Err(WireError::MissingProof);
        }
        Ok(())
    }

    fn native_dispatch_admit(
        &mut self,
        session: WireSession,
        key: WireKey,
        effect: &WireEffect,
    ) -> Result<Response, WireError> {
        let WireEffect::SampleResources { sequence } = *effect else {
            return Err(WireError::UnsupportedDurableBinding);
        };
        let session = self.resolve_session(session)?;
        let key = self.resolve_key(key)?;
        self.validate_native_sampling_evidence(session, key, sequence)?;
        if self
            .native_admissions
            .values()
            .any(|admission| admission.key() == key)
        {
            return Err(WireError::AdmissionReplayed);
        }
        let now = native_reconciliation_now()?;
        self.guardian
            .validate_native_sampling_admission(session, key, sequence, now)
            .map_err(WireError::from)?;
        let admission = NativeSamplingAdmission {
            session,
            key,
            sequence,
        };
        let handle = self.next_admission;
        self.next_admission = handle.checked_add(1).ok_or(WireError::InvalidValue)?;
        self.native_admissions.insert(handle, admission);
        Ok(Response::Admitted { admission: handle })
    }

    fn native_dispatch_submit(
        &mut self,
        session: WireSession,
        admission: u64,
        _now: u64,
    ) -> Result<Response, WireError> {
        // Consume first even on a mismatched session or failed execution.
        let admission = self
            .native_admissions
            .remove(&admission)
            .ok_or(WireError::UnknownAdmission)?;
        let session = self.resolve_session(session)?;
        if admission.session() != session {
            return Err(WireError::AdmissionMismatch);
        }
        self.validate_native_sampling_evidence(session, admission.key(), admission.sequence())?;
        let now = native_reconciliation_now()?;
        let origin = (
            admission.sequence(),
            NonZeroU64::new(admission.session().generation()).ok_or(WireError::InvalidValue)?,
        );
        match self.guardian.submit_native_sampling(admission, now) {
            Ok(key) => {
                self.native_retained_origins.push((key, origin.0, origin.1));
                Ok(Response::Submitted {
                    key: key.try_into()?,
                })
            }
            Err(crate::guardian::PolicySubmissionError::Refused { error, .. }) => Err(error.into()),
            Err(crate::guardian::PolicySubmissionError::ExecutorCorrelationMismatch) => {
                Err(GuardianError::ExecutorCorrelationMismatch.into())
            }
        }
    }

    fn dispatch_admit(
        &mut self,
        session: WireSession,
        key: WireKey,
        effect: &WireEffect,
    ) -> Result<Response, WireError> {
        Ok({
            let session = self.resolve_session(session)?;
            self.guardian
                .authenticate_policy_session(
                    session,
                    self.guardian.attempt_id(),
                    self.guardian.original_deadline(),
                )
                .map_err(WireError::from)?;
            if self
                .admissions
                .values()
                .any(|(_, ticket)| WireKey::try_from(ticket.key()).ok() == Some(key))
            {
                return Err(WireError::AdmissionReplayed);
            }
            let position = self
                .staged
                .iter()
                .position(|ticket| {
                    WireKey::try_from(ticket.key()).ok() == Some(key)
                        && WireEffect::from(ticket.effect()) == *effect
                })
                .ok_or(WireError::NoStagedTicket)?;
            if self
                .admissions
                .values()
                .any(|(_, ticket)| ticket.key() == self.staged[position].key())
            {
                return Err(WireError::AdmissionReplayed);
            }
            let handle = self.next_admission;
            self.next_admission = self
                .next_admission
                .checked_add(1)
                .ok_or(WireError::InvalidValue)?;
            self.admissions
                .insert(handle, (session, self.staged.swap_remove(position)));
            Response::Admitted { admission: handle }
        })
    }
    fn dispatch_submit(
        &mut self,
        session: WireSession,
        admission: u64,
        now: u64,
    ) -> Result<Response, WireError> {
        Ok({
            let session = self.resolve_session(session)?;
            // Remove BEFORE execution; an ambiguous lost response cannot issue this handle twice.
            let (bound_session, ticket) = self
                .admissions
                .remove(&admission)
                .ok_or(WireError::UnknownAdmission)?;
            if bound_session != session {
                self.refused.push(ticket);
                return Err(WireError::AdmissionMismatch);
            }
            match self
                .guardian
                .submit_policy_ticket(session, ticket, MonotonicTick::new(now))
            {
                Ok(key) => Response::Submitted {
                    key: key.try_into()?,
                },
                Err(crate::guardian::PolicySubmissionError::Refused { error, ticket }) => {
                    self.refused.push(ticket);
                    return Err(error.into());
                }
                Err(crate::guardian::PolicySubmissionError::ExecutorCorrelationMismatch) => {
                    return Err(GuardianError::ExecutorCorrelationMismatch.into());
                }
            }
        })
    }
    #[allow(clippy::too_many_lines)]
    fn try_handle(&mut self, request: Request) -> Result<Response, WireError> {
        Ok(match request {
            Request::Hello { .. } => Response::Hello {
                build: self.build.clone(),
                snapshot: self.snapshot(),
            },
            Request::Snapshot => Response::Snapshot(self.snapshot()),
            Request::ReleaseInitial {
                session,
                attempt,
                nonce,
                attempt_revision,
            } => self.release_initial(session, attempt, nonce, attempt_revision)?,
            Request::Inventory => {
                Response::Inventory(self.guardian.inventory().map(Into::into).collect())
            }
            Request::Fence { session } => {
                let session = self.resolve_session(session)?;
                self.guardian
                    .controller_lost(session)
                    .map_err(WireError::from)?;
                self.initial_gate.take();
                self.staged.clear();
                self.refused.clear();
                self.admissions.clear();
                self.native_admissions.clear();
                self.interruption = None;
                self.finish_proof = None;
                self.prepared_activation = None;
                self.ack_proofs.clear();
                Response::Fenced
            }
            Request::BeginReconciliation {
                instance,
                inventory,
                interruption_revision,
            } => self.begin_reconciliation(instance, inventory, interruption_revision)?,
            Request::FinishReconciliation {
                session,
                applied_revision,
                now,
            } => self.finish_reconciliation(session, applied_revision, now)?,
            Request::ActivateReconciliation {
                session,
                attempt,
                recovery_revision,
                binding_revision,
            } => self.activate_native_reconciliation(
                session,
                attempt,
                recovery_revision,
                binding_revision,
            )?,
            Request::ObserveSlot { slot } => {
                let slot = self.slot(slot)?;
                Response::SlotObserved(
                    self.guardian
                        .poll_observation_only(slot)
                        .map_err(WireError::from)?
                        .into(),
                )
            }
            Request::ObserveRetained { key } => {
                let key = self.resolve_key(key)?;
                Response::Retained(self.guardian.retained_result(key).map(WireRetained::from))
            }
            Request::VerifyRecoveredNativeSampling {
                session,
                key,
                sequence,
                issuing_session_generation,
            } => self.verify_recovered_native_sampling(
                session,
                key,
                sequence,
                issuing_session_generation,
            )?,
            Request::AckAfterAppliedRevision {
                session,
                key,
                applied_revision,
            } => self.ack_after_applied_revision(session, key, applied_revision)?,
            Request::ReserveSignalRound {
                session,
                revision: rev,
                count,
            } => {
                let session = self.resolve_session(session)?;
                self.guardian
                    .reserve_signal_results(session, revision(rev)?, count as usize)
                    .map_err(WireError::from)?;
                Response::SignalRoundReserved
            }
            Request::DispatchAdmit {
                session,
                key,
                effect,
            } => {
                if self.native_unbound {
                    self.native_dispatch_admit(session, key, &effect)?
                } else {
                    self.dispatch_admit(session, key, &effect)?
                }
            }
            Request::DispatchSubmit {
                session,
                admission,
                now,
            } => {
                if self.native_unbound {
                    self.native_dispatch_submit(session, admission, now)?
                } else {
                    self.dispatch_submit(session, admission, now)?
                }
            }
        })
    }
}
/// Handle one request on a Unix-domain stream. Any framing/transport error
/// fences the session; a caller must reconcile rather than guessing whether
/// a submitted effect ran before its response was lost.
///
/// # Errors
/// Returns a framing, serialization, or stream I/O error from reading the
/// request or writing its response. On error, the server fences the session.
pub fn serve_once<E: GuardianExecutor>(
    stream: &mut std::os::unix::net::UnixStream,
    server: &mut GuardianServer<E>,
) -> Result<(), CodecError> {
    let result = (|| {
        let request: Request = read_frame(&mut *stream)?;
        write_frame(&mut *stream, &server.handle(request))
    })();
    if result.is_err() {
        server.fence_connection();
    }
    result
}

fn revision(value: u64) -> Result<PersistenceRevision, WireError> {
    NonZeroU64::new(value)
        .map(PersistenceRevision::new)
        .ok_or(WireError::InvalidValue)
}

impl From<&ExecutedEffect> for WireRetained {
    fn from(value: &ExecutedEffect) -> Self {
        Self {
            key: value
                .key()
                .try_into()
                .expect("usize fits u64 on supported hosts"),
            completion: WireCompletion::from(value.completion()),
        }
    }
}
impl From<crate::ObservationFailure> for WireObservationFailure {
    fn from(value: crate::ObservationFailure) -> Self {
        match value {
            crate::ObservationFailure::NoSuchProcess => Self::NoSuchProcess,
            crate::ObservationFailure::PermissionDenied => Self::PermissionDenied,
            crate::ObservationFailure::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            crate::ObservationFailure::Malformed => Self::Malformed,
        }
    }
}
fn wire_resource_sample(sample: &crate::ResourceSample) -> WireResourceSample {
    WireResourceSample {
        sequence: sample.sequence,
        memory_bytes: sample.capacity.available_memory_bytes,
        storage_bytes: sample.capacity.available_storage_bytes,
        cpu_basis_points: sample.contention.cpu.get(),
        memory_pressure: match sample.contention.memory_pressure {
            crate::MemoryPressure::Nominal => WireMemoryPressure::Normal,
            crate::MemoryPressure::Warning => WireMemoryPressure::Elevated,
            crate::MemoryPressure::Critical => WireMemoryPressure::Critical,
        },
    }
}
fn wire_child_observation(
    observation: &crate::ChildIdentityObservation,
) -> WireChildIdentityObservation {
    match observation {
        crate::ChildIdentityObservation::Stable(evidence) => WireChildIdentityObservation::Stable {
            identity: evidence.identity().into(),
        },
        crate::ChildIdentityObservation::Unresolved(error) => {
            WireChildIdentityObservation::Unresolved((*error).into())
        }
        crate::ChildIdentityObservation::DifferentProcess(identity) => {
            WireChildIdentityObservation::DifferentProcess((*identity).into())
        }
    }
}
impl From<&EffectCompletion> for WireCompletion {
    fn from(value: &EffectCompletion) -> Self {
        match value {
            EffectCompletion::SampleResources { sequence, result } => Self::SampleResources {
                sequence: *sequence,
                result: result
                    .as_ref()
                    .map(wire_resource_sample)
                    .map_err(|err| match err {
                        crate::ResourceObservationFailure::TemporarilyUnavailable => {
                            WireResourceError::TemporarilyUnavailable
                        }
                        crate::ResourceObservationFailure::Unsupported => {
                            WireResourceError::Unsupported
                        }
                        crate::ResourceObservationFailure::Malformed => {
                            WireResourceError::Malformed
                        }
                    }),
            },
            EffectCompletion::ListChildren { parent, result } => Self::ListChildren {
                parent: (*parent).into(),
                result: result
                    .as_ref()
                    .map(|pids| pids.iter().map(|pid| pid.get()).collect())
                    .map_err(|err| match err {
                        crate::OsQueryError::PermissionDenied => WireQueryError::PermissionDenied,
                        crate::OsQueryError::TemporarilyUnavailable => {
                            WireQueryError::TemporarilyUnavailable
                        }
                        crate::OsQueryError::Malformed => WireQueryError::Malformed,
                    }),
            },
            EffectCompletion::AcquireChildIdentity {
                discovery,
                observation,
            } => Self::AcquireChildIdentity {
                discovery: (*discovery).into(),
                observation: wire_child_observation(observation),
            },
            EffectCompletion::ObserveIdentity {
                identity,
                observation,
            } => Self::ObserveIdentity {
                identity: (*identity).into(),
                observation: match observation {
                    crate::IdentityObservation::SameIdentity { current_parent } => {
                        WireIdentityObservation::SameIdentity {
                            current_parent: current_parent.map(Into::into),
                        }
                    }
                    crate::IdentityObservation::ExactIdentityAbsent => {
                        WireIdentityObservation::ExactIdentityAbsent
                    }
                    crate::IdentityObservation::PidReused(value) => {
                        WireIdentityObservation::PidReused((*value).into())
                    }
                    crate::IdentityObservation::Unresolved(error) => {
                        WireIdentityObservation::Unresolved((*error).into())
                    }
                },
            },
            EffectCompletion::SignalExactIdentity {
                identity,
                signal,
                outcome,
            } => Self::SignalExactIdentity {
                identity: (*identity).into(),
                signal: (*signal).into(),
                outcome: match outcome {
                    crate::SignalOutcome::Delivered => WireSignalOutcome::Delivered,
                    crate::SignalOutcome::ExactIdentityAlreadyAbsent => {
                        WireSignalOutcome::ExactIdentityAlreadyAbsent
                    }
                    crate::SignalOutcome::PidReused => WireSignalOutcome::PidReused,
                    crate::SignalOutcome::PermissionDenied => WireSignalOutcome::PermissionDenied,
                    crate::SignalOutcome::TemporarilyUnavailable => {
                        WireSignalOutcome::TemporarilyUnavailable
                    }
                },
            },
            EffectCompletion::ReverifyRetainedCandidate {
                candidate,
                verification,
            } => Self::ReverifyRetainedCandidate {
                commit: candidate.source_commit.bytes(),
                digest: candidate.artifact_digest.bytes(),
                verification: match verification {
                    crate::CandidateVerification::Exact => WireCandidateVerification::Exact,
                    crate::CandidateVerification::Missing => WireCandidateVerification::Missing,
                    crate::CandidateVerification::Mismatch => WireCandidateVerification::Mismatch,
                    crate::CandidateVerification::Unavailable => {
                        WireCandidateVerification::Unavailable
                    }
                },
            },
        }
    }
}

#[cfg(test)]
fn test_build() -> BuildIdentity {
    BuildIdentity::parse("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        cell::Cell,
        collections::BTreeMap,
        io::{Cursor, Read},
        rc::Rc,
    };

    struct OneByte<R>(R);
    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = buf.len().min(1);
            self.0.read(&mut buf[..count])
        }
    }
    #[test]
    fn fragmented_round_trip_and_malformed_frames() {
        let request = Request::ReserveSignalRound {
            session: WireSession {
                instance: 12,
                generation: 3,
            },
            revision: 7,
            count: 2,
        };
        let mut framed = Vec::new();
        write_frame(&mut framed, &request).unwrap();
        assert_eq!(
            read_frame::<Request>(&mut OneByte(Cursor::new(&framed))).unwrap(),
            request
        );
        assert!(
            matches!(read_frame::<Request>(&mut Cursor::new([0, 1])), Err(CodecError::Io(err)) if err.kind() == io::ErrorKind::UnexpectedEof)
        );
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new([0, 0, 0, 0])),
            Err(CodecError::Empty)
        ));
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(
                u32::try_from(MAX_FRAME_BYTES + 1)
                    .expect("frame limit plus one fits u32")
                    .to_be_bytes()
            )),
            Err(CodecError::Oversized)
        ));
        let mut truncated = framed.clone();
        truncated.pop();
        assert!(
            matches!(read_frame::<Request>(&mut Cursor::new(truncated)), Err(CodecError::Io(err)) if err.kind() == io::ErrorKind::UnexpectedEof)
        );
        let mut malformed = Vec::new();
        malformed.extend_from_slice(&3_u32.to_be_bytes());
        malformed.extend_from_slice(b"{{{");
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(malformed)),
            Err(CodecError::Malformed(_))
        ));
        let mut unknown = Vec::new();
        let unsupported = PROTOCOL_VERSION.checked_add(1).unwrap();
        let bytes = serde_json::to_string(&Envelope {
            version: unsupported,
            body: Request::Hello {
                build: test_build(),
            },
        })
        .unwrap();
        unknown.extend_from_slice(
            &u32::try_from(bytes.len())
                .expect("fixed test JSON fits u32")
                .to_be_bytes(),
        );
        unknown.extend_from_slice(bytes.as_bytes());
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(unknown)),
            Err(CodecError::UnsupportedVersion(version)) if version == unsupported
        ));
        assert_eq!(PROTOCOL_VERSION, 5);
        let old = serde_json::to_vec(&Envelope {
            version: 4,
            body: Request::Snapshot,
        })
        .unwrap();
        let mut old_frame = u32::try_from(old.len()).unwrap().to_be_bytes().to_vec();
        old_frame.extend(old);
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(old_frame)),
            Err(CodecError::UnsupportedVersion(4))
        ));
        let mut unknown_op = Vec::new();
        let bytes =
            format!(r#"{{"version":{PROTOCOL_VERSION},"body":{{"op":"execute_arbitrary"}}}}"#);
        unknown_op.extend_from_slice(
            &u32::try_from(bytes.len())
                .expect("fixed test JSON fits u32")
                .to_be_bytes(),
        );
        unknown_op.extend_from_slice(bytes.as_bytes());
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(unknown_op)),
            Err(CodecError::Malformed(_))
        ));
        assert!(matches!(
            write_frame(&mut Vec::new(), &"x".repeat(MAX_FRAME_BYTES)),
            Err(CodecError::Oversized)
        ));
    }
    struct Fake {
        calls: Rc<Cell<u32>>,
    }
    impl GuardianExecutor for Fake {
        type ChildSlot = ();
        fn execute(
            &mut self,
            ticket: DispatchTicket,
            _: &mut BTreeMap<crate::ChildSlotId, ()>,
        ) -> ExecutedEffect {
            self.calls.set(self.calls.get() + 1);
            let sequence = match ticket.effect() {
                OsEffect::SampleResources { sequence } => *sequence,
                OsEffect::ListChildren { .. }
                | OsEffect::AcquireChildIdentity { .. }
                | OsEffect::ObserveIdentity { .. }
                | OsEffect::SignalExactIdentity { .. }
                | OsEffect::ReverifyRetainedCandidate { .. } => panic!("fixture only samples"),
            };
            ExecutedEffect::from_parts(
                ticket.key(),
                EffectCompletion::SampleResources {
                    sequence,
                    result: Err(crate::ResourceObservationFailure::Unsupported),
                },
            )
        }
        fn poll_observation_only(&self, &(): &()) -> ChildSlotObservation {
            ChildSlotObservation::Running
        }
    }
    fn fixture() -> (GuardianServer<Fake>, Rc<Cell<u32>>, DispatchTicket) {
        let calls = Rc::new(Cell::new(0));
        let (guardian, session) = Guardian::new(
            crate::AttemptId::new(8),
            MonotonicTick::new(100),
            Fake {
                calls: calls.clone(),
            },
            vec![()],
        )
        .unwrap();
        let ticket = DispatchTicket::from_committed_parts(
            DispatchKey::from_committed_parts(
                crate::AttemptId::new(8),
                PersistenceRevision::first(),
                0,
            ),
            OsEffect::SampleResources { sequence: 4 },
        );
        (
            GuardianServer::new(guardian, session, test_build()).unwrap(),
            calls,
            ticket,
        )
    }
    #[test]
    fn build_handshake_precedes_all_requests_and_checks_exact_echo() {
        let (mut server, calls, _) = fixture();
        assert_eq!(
            server.handle(Request::Snapshot),
            Response::Refused(WireError::BuildMismatch)
        );
        let other = BuildIdentity::parse(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();
        assert_eq!(
            server.handle(Request::Hello { build: other }),
            Response::Refused(WireError::BuildMismatch)
        );
        let Response::Hello { build, snapshot } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("expected verified hello")
        };
        assert_eq!(build, test_build());
        assert_eq!(snapshot.attempt, 8);
        assert_eq!(calls.get(), 0);
        server.fence_connection();
        assert_eq!(
            server.handle(Request::Snapshot),
            Response::Refused(WireError::BuildMismatch)
        );
    }

    #[test]
    fn wire_build_identity_requires_full_lowercase_hex() {
        for invalid in [
            "",
            "01234567",
            "ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "g123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            assert!(BuildIdentity::parse(invalid).is_err());
            assert!(serde_json::from_value::<BuildIdentity>(serde_json::json!(invalid)).is_err());
        }
    }

    #[test]
    fn admission_is_exact_session_instance_key_effect_and_single_use() {
        let (mut server, calls, ticket) = fixture();
        let key = ticket.key().try_into().unwrap();
        let effect: WireEffect = ticket.effect().into();
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        let session = snapshot.session;
        server.stage_committed(ticket).unwrap();
        let admit = |session, key, effect| Request::DispatchAdmit {
            session,
            key,
            effect,
        };
        assert_eq!(
            server.handle(admit(
                WireSession {
                    instance: session.instance + 1,
                    ..session
                },
                key,
                effect.clone()
            )),
            Response::Refused(GuardianError::InstanceMismatch.into())
        );
        assert_eq!(
            server.handle(admit(
                WireSession {
                    generation: session.generation + 1,
                    ..session
                },
                key,
                effect.clone()
            )),
            Response::Refused(GuardianError::StaleSession.into())
        );
        assert_eq!(
            server.handle(admit(
                session,
                WireKey { ordinal: 1, ..key },
                effect.clone()
            )),
            Response::Refused(WireError::NoStagedTicket)
        );
        assert_eq!(
            server.handle(admit(
                session,
                key,
                WireEffect::SampleResources { sequence: 5 }
            )),
            Response::Refused(WireError::NoStagedTicket)
        );
        let Response::Admitted { admission } = server.handle(admit(session, key, effect.clone()))
        else {
            panic!("admit")
        };
        assert_eq!(
            server.handle(admit(session, key, effect)),
            Response::Refused(WireError::AdmissionReplayed)
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 1
            }),
            Response::Submitted { key }
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 1
            }),
            Response::Refused(WireError::UnknownAdmission)
        );
        assert_eq!(calls.get(), 1);
        let retained = server.handle(Request::ObserveRetained { key });
        let mut framed = Vec::new();
        write_frame(&mut framed, &retained).unwrap();
        assert_eq!(
            read_frame::<Response>(&mut Cursor::new(framed)).unwrap(),
            retained
        );
        assert_eq!(
            server.handle(Request::AckAfterAppliedRevision {
                session,
                key,
                applied_revision: 1
            }),
            Response::Refused(WireError::MissingProof)
        );
        assert!(matches!(
            server.handle(Request::ObserveRetained { key }),
            Response::Retained(Some(_))
        ));
    }
    #[test]
    fn unix_stream_disconnect_fences_without_execution() {
        use std::os::unix::net::UnixStream;
        let (mut server, calls, _) = fixture();
        let (mut controller, mut guardian_socket) = UnixStream::pair().unwrap();
        write_frame(
            &mut controller,
            &Request::Hello {
                build: test_build(),
            },
        )
        .unwrap();
        serve_once(&mut guardian_socket, &mut server).unwrap();
        assert!(matches!(
            read_frame::<Response>(&mut controller).unwrap(),
            Response::Hello { .. }
        ));
        drop(controller);
        assert!(matches!(
            serve_once(&mut guardian_socket, &mut server),
            Err(CodecError::Io(_))
        ));
        assert_eq!(server.guardian().state(), GuardianState::DetachedFenced);
        assert_eq!(calls.get(), 0);
    }
    #[test]
    fn definite_pre_execution_refusal_never_restages_for_wire_replay() {
        let (mut server, calls, ticket) = fixture();
        let key = ticket.key().try_into().unwrap();
        let effect: WireEffect = ticket.effect().into();
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        let session = snapshot.session;
        server.stage_committed(ticket).unwrap();
        let Response::Admitted { admission } = server.handle(Request::DispatchAdmit {
            session,
            key,
            effect: effect.clone(),
        }) else {
            panic!("admit")
        };
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 100
            }),
            Response::Refused(GuardianError::DeadlineExpired.into())
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 1
            }),
            Response::Refused(WireError::UnknownAdmission)
        );
        assert_eq!(
            server.handle(Request::DispatchAdmit {
                session,
                key,
                effect
            }),
            Response::Refused(GuardianError::AuthorityLost.into())
        );
        let refused = server.take_refused();
        assert_eq!(refused.len(), 1);
        assert_eq!(
            refused[0].key(),
            DispatchKey::from_committed_parts(
                crate::AttemptId::new(8),
                PersistenceRevision::first(),
                0
            )
        );
    }
    #[test]
    fn stale_reconciliation_and_fence_require_trusted_interruption() {
        let (mut server, calls, _) = fixture();
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        let old = snapshot.session;
        let instance = snapshot.instance;
        let inventory = snapshot.slots;
        assert_eq!(
            server.handle(Request::Fence { session: old }),
            Response::Fenced
        );
        let begin = |instance, inventory| Request::BeginReconciliation {
            instance,
            inventory,
            interruption_revision: 1,
        };
        assert_eq!(
            server.handle(begin(instance + 1, inventory.clone())),
            Response::Refused(GuardianError::InstanceMismatch.into())
        );
        assert_eq!(
            server.handle(begin(instance, inventory.clone())),
            Response::Refused(WireError::MissingProof)
        );
        server.stage_interruption(PersistenceRevision::first());
        let Response::ReconciliationBegun { session } = server.handle(begin(instance, inventory))
        else {
            panic!("begin")
        };
        assert_eq!(
            server.handle(Request::FinishReconciliation {
                session: old,
                applied_revision: 1,
                now: 1
            }),
            Response::Refused(GuardianError::StaleSession.into())
        );
        assert_eq!(
            server.handle(Request::FinishReconciliation {
                session,
                applied_revision: 1,
                now: 1
            }),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(calls.get(), 0);
    }
}

#[cfg(all(test, unix))]
mod native_evidence_tests {
    use super::*;
    use crate::{
        AttemptId, BirthToken, ControllerSqliteStore, CpuContentionBasisPoints, DeploymentAttempt,
        GuardianLifetimeNonce, MemoryPressure, NativeGuardianBinding, NativeGuardianIdentity,
        NativeSessionState, Phase, ResourcePolicy,
    };
    use std::{
        collections::BTreeMap,
        io::Cursor,
        num::{NonZeroU32, NonZeroUsize},
    };

    struct Observer;
    impl GuardianExecutor for Observer {
        type ChildSlot = ProcessIdentity;
        fn execute(
            &mut self,
            _: DispatchTicket,
            _: &mut BTreeMap<crate::ChildSlotId, Self::ChildSlot>,
        ) -> ExecutedEffect {
            panic!("read-only guardian")
        }
        fn poll_observation_only(&self, _: &Self::ChildSlot) -> ChildSlotObservation {
            ChildSlotObservation::Running
        }
        fn diagnostic_slot_identity(&self, slot: &Self::ChildSlot) -> Option<ProcessIdentity> {
            Some(*slot)
        }
    }
    struct Reader(Option<(crate::LoadedAttempt, NativeGuardianBinding)>);
    impl NativeEvidenceReader for Reader {
        fn activation_evidence(
            &mut self,
            _: crate::AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                crate::NativeGuardianBinding,
                crate::NativeReconciliationActivation,
            )>,
            NativeEvidenceReadError,
        > {
            Err(NativeEvidenceReadError)
        }
        fn reconciliation_evidence(
            &mut self,
            _: AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeReconciliationCompletion,
            )>,
            NativeEvidenceReadError,
        > {
            Ok(None)
        }
        fn read(
            &mut self,
            _: AttemptId,
        ) -> Result<Option<(crate::LoadedAttempt, NativeGuardianBinding)>, NativeEvidenceReadError>
        {
            Ok(self.0.take())
        }
        fn sampling_evidence(
            &mut self,
            _: DispatchKey,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                crate::NativeGuardianBinding,
                crate::NativeSamplingStatus,
            )>,
            NativeEvidenceReadError,
        > {
            Ok(None)
        }
    }
    struct SamplingReader {
        store: ControllerSqliteStore,
        status: std::sync::Arc<std::sync::Mutex<Option<crate::NativeSamplingStatus>>>,
    }
    impl NativeEvidenceReader for SamplingReader {
        fn activation_evidence(
            &mut self,
            _: crate::AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                crate::NativeGuardianBinding,
                crate::NativeReconciliationActivation,
            )>,
            NativeEvidenceReadError,
        > {
            Err(NativeEvidenceReadError)
        }
        fn reconciliation_evidence(
            &mut self,
            attempt: AttemptId,
            generation: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeReconciliationCompletion,
            )>,
            NativeEvidenceReadError,
        > {
            self.store
                .read_native_reconciliation_evidence(attempt, generation)
                .map_err(|_| NativeEvidenceReadError)
        }
        fn read(
            &mut self,
            attempt: AttemptId,
        ) -> Result<Option<(crate::LoadedAttempt, NativeGuardianBinding)>, NativeEvidenceReadError>
        {
            self.store
                .read_native_evidence(attempt)
                .map_err(|_| NativeEvidenceReadError)
        }
        fn sampling_evidence(
            &mut self,
            key: DispatchKey,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                crate::NativeGuardianBinding,
                crate::NativeSamplingStatus,
            )>,
            NativeEvidenceReadError,
        > {
            let evidence = self
                .store
                .read_native_sampling_evidence(key)
                .map_err(|_| NativeEvidenceReadError)?;
            let status = *self.status.lock().unwrap();
            Ok(match (evidence, status) {
                (Some((checkpoint, binding, _)), Some(status)) => {
                    Some((checkpoint, binding, status))
                }
                (evidence, None) => evidence,
                (None, Some(_)) => None,
            })
        }
    }

    struct SamplingExecutor(std::sync::Arc<std::sync::atomic::AtomicUsize>, bool);
    impl GuardianExecutor for SamplingExecutor {
        type ChildSlot = ProcessIdentity;
        fn execute(
            &mut self,
            ticket: DispatchTicket,
            _: &mut BTreeMap<crate::ChildSlotId, ProcessIdentity>,
        ) -> ExecutedEffect {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let OsEffect::SampleResources { sequence } = ticket.effect() else {
                panic!("only sampling")
            };
            ExecutedEffect::from_parts(
                ticket.key(),
                EffectCompletion::SampleResources {
                    sequence: *sequence,
                    result: if self.1 {
                        Ok(crate::ResourceSample {
                            sequence: *sequence,
                            capacity: crate::ResourceCapacity {
                                available_memory_bytes: 11,
                                available_storage_bytes: 12,
                            },
                            contention: crate::ResourceContention {
                                cpu: CpuContentionBasisPoints::new(42).unwrap(),
                                memory_pressure: MemoryPressure::Nominal,
                            },
                        })
                    } else {
                        Err(crate::ResourceObservationFailure::Unsupported)
                    },
                },
            )
        }
        fn poll_observation_only(&self, _: &ProcessIdentity) -> ChildSlotObservation {
            ChildSlotObservation::Running
        }
        fn diagnostic_slot_identity(&self, slot: &ProcessIdentity) -> Option<ProcessIdentity> {
            Some(*slot)
        }
    }

    type SamplingFixture = (
        GuardianServer<SamplingExecutor>,
        crate::NativeSamplingIssue,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::sync::Arc<std::sync::Mutex<Option<crate::NativeSamplingStatus>>>,
    );

    fn sampling_fixture() -> SamplingFixture {
        sampling_fixture_with_status(false)
    }

    fn sampling_fixture_with_status(wrong_status: bool) -> SamplingFixture {
        let attempt = AttemptId::new(920);
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(std::time::Duration::from_secs(120))
            .unwrap();
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(0).unwrap(),
            MemoryPressure::Critical,
        );
        let committed = DeploymentAttempt::start(attempt, deadline, policy, identity(41))
            .persist(&mut store)
            .unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (guardian, session) = Guardian::new(
            attempt,
            deadline,
            SamplingExecutor(calls.clone(), false),
            vec![identity(42)],
        )
        .unwrap();
        let mut server = GuardianServer::new(guardian, session, test_build()).unwrap();
        server.stage_native_identity(NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([8; 16]).unwrap(),
            process: identity(43),
        });
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        crate::native_guardian::verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        let binding = store.load_native_binding(attempt).unwrap().unwrap();
        let mut policy = crate::PolicyLoop::from_committed(committed);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        let status = wrong_status.then_some(crate::NativeSamplingStatus::ParkedRetained {
            issue,
            failure: crate::ResourceObservationFailure::Unsupported,
            applying_revision: PersistenceRevision::new(NonZeroU64::new(2).unwrap()),
        });
        let status = std::sync::Arc::new(std::sync::Mutex::new(status));
        server.stage_native_reader(Box::new(SamplingReader {
            store,
            status: status.clone(),
        }));
        (server, issue, calls, status)
    }

    #[test]
    fn native_sampling_admission_executes_once_and_retains_exact_completion() {
        let (mut server, issue, calls, _status) = sampling_fixture();
        let key = WireKey::try_from(issue.key()).unwrap();
        let session = server.snapshot().session;
        let admit = |session, key, sequence| Request::DispatchAdmit {
            session,
            key,
            effect: WireEffect::SampleResources { sequence },
        };
        let Response::Admitted { admission } = server.handle(admit(session, key, issue.sequence()))
        else {
            panic!("admit")
        };
        assert_eq!(
            server.handle(admit(session, key, issue.sequence())),
            Response::Refused(WireError::AdmissionReplayed)
        );
        let submit = || Request::DispatchSubmit {
            session,
            admission,
            now: 0,
        };
        assert_eq!(server.handle(submit()), Response::Submitted { key });
        assert_eq!(
            server.handle(submit()),
            Response::Refused(WireError::UnknownAdmission)
        );
        assert!(matches!(
            server.handle(admit(session, key, issue.sequence())),
            Response::Refused(WireError::Guardian(WireGuardianError::DuplicateDispatchKey))
        ));
        assert_eq!(
            server.handle(Request::ObserveRetained { key }),
            Response::Retained(Some(WireRetained {
                key,
                completion: WireCompletion::SampleResources {
                    sequence: issue.sequence(),
                    result: Err(WireResourceError::Unsupported),
                },
            }))
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            server.handle(Request::AckAfterAppliedRevision {
                session,
                key,
                applied_revision: key.revision
            }),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(
            server.handle(Request::ReserveSignalRound {
                session,
                revision: key.revision,
                count: 1
            }),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
    }

    struct NativeAckFixture {
        _directory: tempfile::TempDir,
        path: std::path::PathBuf,
        attempt: AttemptId,
        deadline: MonotonicTick,
        server: GuardianServer<SamplingExecutor>,
        issue: crate::NativeSamplingIssue,
        ticket: Option<DispatchTicket>,
        policy: crate::PolicyLoop,
        store: ControllerSqliteStore,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        active_session: WireSession,
        key: WireKey,
        binding: NativeGuardianBinding,
    }

    fn native_ack_fixture() -> NativeAckFixture {
        native_ack_fixture_with_options(true, std::time::Duration::from_secs(120))
    }

    fn native_ack_fixture_with_options(
        successful: bool,
        remaining: std::time::Duration,
    ) -> NativeAckFixture {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controller.sqlite");
        let attempt = AttemptId::new(921);
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(remaining)
            .unwrap();
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        );
        let committed = DeploymentAttempt::start(attempt, deadline, policy, identity(41))
            .persist(&mut store)
            .unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (guardian, session) = Guardian::new(
            attempt,
            deadline,
            SamplingExecutor(calls.clone(), successful),
            vec![identity(42)],
        )
        .unwrap();
        let mut server = GuardianServer::new(guardian, session, test_build()).unwrap();
        server.stage_native_identity(NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([8; 16]).unwrap(),
            process: identity(43),
        });
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        crate::native_guardian::verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        let binding = store.load_native_binding(attempt).unwrap().unwrap();
        let mut policy = crate::PolicyLoop::from_committed(committed);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        let key = WireKey::try_from(issue.key()).unwrap();
        server.stage_native_reader(Box::new(
            crate::native_guardian::SqliteNativeEvidenceReader::open(&path, attempt).unwrap(),
        ));
        NativeAckFixture {
            _directory: directory,
            path,
            attempt,
            deadline,
            server,
            issue,
            ticket: Some(ticket),
            policy,
            store,
            calls,
            active_session: snapshot.session,
            key,
            binding,
        }
    }

    fn dispatch_and_apply_native_sample(
        fixture: &mut NativeAckFixture,
    ) -> crate::NativeSamplingStatus {
        use crate::{
            NativeSamplingCompletion, NativeSamplingDispatch, ResourceCapacity, ResourceContention,
            ResourceSample, VerifiedNativeSamplingCompletion,
        };
        let Response::Admitted { admission } = fixture.server.handle(Request::DispatchAdmit {
            session: fixture.active_session,
            key: fixture.key,
            effect: WireEffect::SampleResources {
                sequence: fixture.issue.sequence(),
            },
        }) else {
            panic!("admit")
        };
        assert_eq!(
            fixture.server.handle(Request::DispatchSubmit {
                session: fixture.active_session,
                admission,
                now: 0
            }),
            Response::Submitted { key: fixture.key }
        );
        let sample = ResourceSample {
            sequence: fixture.issue.sequence(),
            capacity: ResourceCapacity {
                available_memory_bytes: 11,
                available_storage_bytes: 12,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(42).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        };
        let verified = VerifiedNativeSamplingCompletion::new(
            NativeSamplingDispatch::new(fixture.ticket.take().unwrap(), fixture.issue),
            NativeSamplingCompletion::Sample(sample),
        );
        fixture
            .policy
            .apply_native_sampling(verified, &mut fixture.store)
            .unwrap();
        fixture
            .store
            .load_native_sampling_status(fixture.issue.key())
            .unwrap()
            .unwrap()
    }

    fn reconnect_ack_session(fixture: &mut NativeAckFixture) -> WireSession {
        fixture.server.fence_connection();
        let Response::Hello {
            snapshot: detached, ..
        } = fixture.server.handle(Request::Hello {
            build: test_build(),
        })
        else {
            panic!("detached hello")
        };
        assert_eq!(detached.state, WireState::DetachedFenced);
        assert_eq!(
            crate::native_guardian::verify_native_binding(
                &mut fixture.store,
                fixture.attempt,
                &detached,
            )
            .unwrap(),
            crate::native_guardian::VerifiedBindingStatus::Detached
        );
        let loaded = fixture
            .store
            .load_attempt(fixture.attempt)
            .unwrap()
            .unwrap();
        let crate::RecoveryDisposition::Checkpoint(interruption) =
            loaded.prepare_recovery().unwrap()
        else {
            panic!("sampling attempt must checkpoint interruption")
        };
        let interruption_revision = (*interruption)
            .persist(&mut fixture.store)
            .unwrap()
            .checkpoint()
            .persistence_revision()
            .unwrap();
        let detached_binding = fixture
            .store
            .load_native_binding(fixture.attempt)
            .unwrap()
            .unwrap();
        let reconciling = fixture
            .store
            .commit_fresh_interruption_binding(&detached_binding, interruption_revision)
            .unwrap();
        let Response::ReconciliationBegun { session } =
            fixture.server.handle(Request::BeginReconciliation {
                instance: detached.instance,
                inventory: detached.slots,
                interruption_revision: interruption_revision.get(),
            })
        else {
            panic!("begin reconciliation")
        };
        assert_eq!(
            session.generation,
            reconciling.current_session().generation.get()
        );
        session
    }

    fn replacement_server_refuses_ack(fixture: &NativeAckFixture, request: Request) {
        let (guardian, session) = Guardian::new(
            fixture.attempt,
            fixture.deadline,
            SamplingExecutor(fixture.calls.clone(), true),
            vec![identity(42)],
        )
        .unwrap();
        let mut replacement = GuardianServer::new(guardian, session, test_build()).unwrap();
        replacement.stage_native_identity(fixture.binding.identity());
        replacement.stage_native_reader(Box::new(
            crate::native_guardian::SqliteNativeEvidenceReader::open(
                &fixture.path,
                fixture.attempt,
            )
            .unwrap(),
        ));
        replacement.handle(Request::Hello {
            build: test_build(),
        });
        assert_eq!(
            replacement.handle(request),
            Response::Refused(WireError::Guardian(WireGuardianError::InstanceMismatch))
        );
    }

    fn commit_sampling_reconciliation(
        fixture: &mut NativeAckFixture,
    ) -> (WireSession, crate::CommittedTransition) {
        let status = dispatch_and_apply_native_sample(fixture);
        let session = reconnect_ack_session(fixture);
        let crate::NativeSamplingStatus::AppliedPendingAck {
            issue,
            sample,
            applying_revision,
        } = status
        else {
            panic!("sample")
        };
        assert_eq!(
            fixture.server.handle(Request::AckAfterAppliedRevision {
                session,
                key: fixture.key,
                applied_revision: applying_revision.get(),
            }),
            Response::Acked {
                key: fixture.key,
                applied_revision: applying_revision.get()
            }
        );
        fixture
            .store
            .confirm_native_sampling_ack(crate::VerifiedNativeSamplingAckConfirmation::new(
                issue,
                sample,
                applying_revision,
                NonZeroU64::new(session.generation).unwrap(),
            ))
            .unwrap();
        let loaded = fixture
            .store
            .load_attempt(fixture.attempt)
            .unwrap()
            .unwrap();
        let binding = fixture
            .store
            .load_native_binding(fixture.attempt)
            .unwrap()
            .unwrap();
        let committed = fixture
            .store
            .commit_native_sampling_recovery_at(loaded, &binding, crate::MonotonicTick::new(0))
            .unwrap();
        (session, committed)
    }

    fn dispatch_native_without_application(fixture: &mut NativeAckFixture) {
        let Response::Admitted { admission } = fixture.server.handle(Request::DispatchAdmit {
            session: fixture.active_session,
            key: fixture.key,
            effect: WireEffect::SampleResources {
                sequence: fixture.issue.sequence(),
            },
        }) else {
            panic!("admission")
        };
        assert_eq!(
            fixture.server.handle(Request::DispatchSubmit {
                session: fixture.active_session,
                admission,
                now: 0,
            }),
            Response::Submitted { key: fixture.key }
        );
        assert_eq!(
            fixture.server.native_retained_origins,
            vec![(
                fixture.issue.key(),
                fixture.issue.sequence(),
                fixture.issue.session_generation()
            )]
        );
    }

    fn recovery_request(fixture: &NativeAckFixture, session: WireSession) -> Request {
        Request::VerifyRecoveredNativeSampling {
            session,
            key: fixture.key,
            sequence: fixture.issue.sequence(),
            issuing_session_generation: fixture.issue.session_generation().get(),
        }
    }

    #[test]
    fn native_recovery_reads_only_exact_pending_sample_after_fence() {
        let mut fixture = native_ack_fixture();
        dispatch_native_without_application(&mut fixture);
        assert!(matches!(
            fixture
                .server
                .handle(recovery_request(&fixture, fixture.active_session)),
            Response::Refused(WireError::MissingProof)
        ));
        let session = reconnect_ack_session(&mut fixture);
        assert_eq!(fixture.server.native_retained_origins.len(), 1);
        let request = recovery_request(&fixture, session);
        let expected = Response::RecoveredNativeSampling {
            key: fixture.key,
            sequence: fixture.issue.sequence(),
            sample: WireResourceSample {
                sequence: fixture.issue.sequence(),
                memory_bytes: 11,
                storage_bytes: 12,
                cpu_basis_points: 42,
                memory_pressure: WireMemoryPressure::Normal,
            },
        };
        assert_eq!(fixture.server.handle(request.clone()), expected);
        assert_eq!(fixture.server.handle(request), expected);
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .store
                .load_native_sampling_status(fixture.issue.key())
                .unwrap(),
            Some(crate::NativeSamplingStatus::IssuedPendingApplication {
                issue: fixture.issue
            })
        );
        let loaded = fixture
            .store
            .load_attempt(fixture.attempt)
            .unwrap()
            .unwrap();
        let candidate = fixture
            .store
            .prepare_native_sampling_recovery(loaded)
            .unwrap();
        let sample = crate::ResourceSample {
            sequence: fixture.issue.sequence(),
            capacity: crate::ResourceCapacity {
                available_memory_bytes: 11,
                available_storage_bytes: 12,
            },
            contention: crate::ResourceContention {
                cpu: CpuContentionBasisPoints::new(42).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        };
        let status = fixture
            .store
            .apply_recovered_native_sampling(
                crate::VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
            )
            .unwrap();
        let crate::NativeSamplingStatus::AppliedPendingAck {
            applying_revision, ..
        } = status
        else {
            panic!("applied")
        };
        assert_eq!(
            fixture.server.handle(recovery_request(&fixture, session)),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(
            fixture.server.handle(Request::AckAfterAppliedRevision {
                session,
                key: fixture.key,
                applied_revision: applying_revision.get(),
            }),
            Response::Acked {
                key: fixture.key,
                applied_revision: applying_revision.get()
            }
        );
        assert!(fixture.server.native_retained_origins.is_empty());
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn native_recovery_refuses_failed_completion_and_expired_deadline() {
        let mut failed =
            native_ack_fixture_with_options(false, std::time::Duration::from_secs(120));
        dispatch_native_without_application(&mut failed);
        let session = reconnect_ack_session(&mut failed);
        assert_eq!(
            failed.server.handle(recovery_request(&failed, session)),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(failed.calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let mut expired = native_ack_fixture_with_options(true, std::time::Duration::from_secs(2));
        dispatch_native_without_application(&mut expired);
        let session = reconnect_ack_session(&mut expired);
        let now = MonotonicTick::native_now().unwrap();
        if now < expired.deadline {
            std::thread::sleep(std::time::Duration::from_nanos(
                expired.deadline.get() - now.get(),
            ));
        }
        assert_eq!(
            expired.server.handle(recovery_request(&expired, session)),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(expired.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn native_recovery_refuses_wrong_origin_key_sequence_session_and_state() {
        for case in 0..8 {
            let mut fixture = native_ack_fixture();
            dispatch_native_without_application(&mut fixture);
            let session = reconnect_ack_session(&mut fixture);
            let mut request = recovery_request(&fixture, session);
            match case {
                0 => {
                    if let Request::VerifyRecoveredNativeSampling {
                        issuing_session_generation,
                        ..
                    } = &mut request
                    {
                        *issuing_session_generation = session.generation;
                    }
                }
                1 => {
                    if let Request::VerifyRecoveredNativeSampling { key, .. } = &mut request {
                        key.ordinal += 1;
                    }
                }
                2 => {
                    if let Request::VerifyRecoveredNativeSampling { sequence, .. } = &mut request {
                        *sequence += 1;
                    }
                }
                3 => {
                    if let Request::VerifyRecoveredNativeSampling { session, .. } = &mut request {
                        session.generation -= 1;
                    }
                }
                4 => {
                    fixture.server.native_retained_origins[0].2 =
                        NonZeroU64::new(session.generation).unwrap();
                }
                5 => {
                    fixture
                        .server
                        .guardian
                        .controller_lost(fixture.server.session)
                        .unwrap();
                }
                6 => {
                    fixture.server.native_identity = None;
                }
                7 => {
                    fixture.server.native_reader = None;
                }
                _ => unreachable!(),
            }
            assert!(
                matches!(fixture.server.handle(request), Response::Refused(_)),
                "case {case}"
            );
            assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn native_finish_reads_atomic_completion_and_never_dispatches_retained_plan() {
        let mut fixture = native_ack_fixture();
        let (session, committed) = commit_sampling_reconciliation(&mut fixture);
        let revision = committed.checkpoint().persistence_revision().unwrap();
        let finish = |session, revision| Request::FinishReconciliation {
            session,
            applied_revision: revision,
            now: 0,
        };
        assert!(matches!(
            fixture
                .server
                .handle(finish(fixture.active_session, revision.get())),
            Response::Refused(_)
        ));
        assert!(matches!(
            fixture.server.handle(finish(session, revision.get() + 1)),
            Response::Refused(_)
        ));
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        let original = fixture.server.snapshot();
        let mut policy = crate::PolicyLoop::from_committed(committed);
        let retained = policy.issue_next().unwrap();
        assert_eq!(
            *retained.effect(),
            OsEffect::SampleResources { sequence: 1 }
        );
        assert_eq!(
            fixture.server.handle(finish(session, revision.get())),
            Response::ReconciliationPrepared {
                session,
                attempt: fixture.attempt.get(),
                recovery_revision: revision.get()
            }
        );
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        let floor = fixture.server.guardian.reconciliation_floor();
        assert!(floor.is_some());
        let binding = fixture
            .store
            .load_native_binding(fixture.attempt)
            .unwrap()
            .unwrap();
        let (_, _, completion) = fixture
            .store
            .read_native_reconciliation_evidence(
                fixture.attempt,
                binding.current_session().generation,
            )
            .unwrap()
            .unwrap();
        let activate = Request::ActivateReconciliation {
            session,
            attempt: fixture.attempt.get(),
            recovery_revision: revision.get(),
            binding_revision: binding.revision().checked_next().unwrap().get(),
        };
        // No durable activation yet, even though a server-local prepared capability exists.
        assert!(matches!(
            fixture.server.handle(activate.clone()),
            Response::Refused(_)
        ));
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        assert!(matches!(
            fixture.server.handle(finish(session, revision.get())),
            Response::Refused(_)
        ));
        let proof = crate::PreparedNativeReconciliation::after_live_prepare(
            session,
            fixture.attempt,
            binding.identity(),
            binding.deadline(),
            revision,
        );
        let (durable, activation, committed) = fixture
            .store
            .commit_native_reconciliation_activation_at(
                &binding,
                completion,
                proof,
                MonotonicTick::native_now().unwrap(),
            )
            .unwrap();
        assert_eq!(durable.current_session().state, NativeSessionState::Active);
        assert_eq!(activation.completion(), completion);
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        assert_eq!(fixture.server.guardian.reconciliation_floor(), floor);
        let (exact_session, attempt, recovery_revision, binding_revision) =
            committed.into_locator();
        assert_eq!(
            activate,
            Request::ActivateReconciliation {
                session: exact_session,
                attempt: attempt.get(),
                recovery_revision: recovery_revision.get(),
                binding_revision: binding_revision.get()
            }
        );
        let mut wrong = activate.clone();
        if let Request::ActivateReconciliation {
            binding_revision, ..
        } = &mut wrong
        {
            *binding_revision += 1;
        }
        assert!(matches!(fixture.server.handle(wrong), Response::Refused(_)));
        assert_eq!(
            fixture.server.handle(activate.clone()),
            Response::ReconciliationFinished
        );
        assert!(matches!(
            fixture.server.handle(activate.clone()),
            Response::Refused(_)
        ));
        let active = fixture.server.snapshot();
        assert_eq!(active.state, WireState::Active);
        assert_eq!(active.instance, original.instance);
        assert_eq!(active.deadline, original.deadline);
        assert_eq!(active.native_binding, original.native_binding);
        assert_eq!(fixture.server.guardian.reconciliation_floor(), None);
        assert!(matches!(
            fixture.server.handle(finish(session, revision.get())),
            Response::Refused(_)
        ));
        fixture.server.fence_connection();
        fixture.server.handle(Request::Hello {
            build: test_build(),
        });
        assert!(matches!(
            fixture.server.handle(finish(session, revision.get())),
            Response::Refused(_)
        ));
        assert_eq!(fixture.server.snapshot().state, WireState::DetachedFenced);
        assert!(fixture.server.prepared_activation.is_none());
        assert!(matches!(
            fixture.server.handle(activate),
            Response::Refused(_)
        ));
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn native_sampling_admission_refuses_prepared_live_reconciling_even_after_durable_activation() {
        let mut fixture = native_ack_fixture();
        let (session, committed) = commit_sampling_reconciliation(&mut fixture);
        let revision = committed.checkpoint().persistence_revision().unwrap();
        let mut policy = crate::PolicyLoop::from_committed(committed);
        let retained = policy.issue_next().unwrap();
        assert_eq!(
            fixture.server.handle(Request::FinishReconciliation {
                session,
                applied_revision: revision.get(),
                now: 0,
            }),
            Response::ReconciliationPrepared {
                session,
                attempt: fixture.attempt.get(),
                recovery_revision: revision.get(),
            }
        );
        let admit = Request::DispatchAdmit {
            session,
            key: WireKey::try_from(retained.key()).unwrap(),
            effect: WireEffect::SampleResources { sequence: 1 },
        };
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        assert_eq!(
            fixture.server.handle(admit.clone()),
            Response::Refused(WireError::MissingProof)
        );
        assert!(fixture.server.native_admissions.is_empty());
        let binding = fixture
            .store
            .load_native_binding(fixture.attempt)
            .unwrap()
            .unwrap();
        let (_, _, completion) = fixture
            .store
            .read_native_reconciliation_evidence(
                fixture.attempt,
                binding.current_session().generation,
            )
            .unwrap()
            .unwrap();
        let proof = crate::PreparedNativeReconciliation::after_live_prepare(
            session,
            fixture.attempt,
            binding.identity(),
            binding.deadline(),
            revision,
        );
        let (durable, _, _activation) = fixture
            .store
            .commit_native_reconciliation_activation_at(
                &binding,
                completion,
                proof,
                MonotonicTick::native_now().unwrap(),
            )
            .unwrap();
        let issue = fixture
            .store
            .commit_native_sampling_issue(&retained, &durable)
            .unwrap();
        assert_eq!(issue.key(), retained.key());
        assert_eq!(issue.sequence(), 1);
        assert_eq!(durable.current_session().state, NativeSessionState::Active);
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        assert_eq!(
            fixture.server.handle(admit),
            Response::Refused(WireError::MissingProof),
            "exact durable issue cannot bypass live Reconciling"
        );
        assert!(fixture.server.native_admissions.is_empty());
        assert!(fixture.server.snapshot().retained.is_empty());
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    struct ReconciliationReader(
        Option<(
            crate::LoadedAttempt,
            NativeGuardianBinding,
            crate::NativeReconciliationCompletion,
        )>,
    );
    impl NativeEvidenceReader for ReconciliationReader {
        fn activation_evidence(
            &mut self,
            _: crate::AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                crate::NativeGuardianBinding,
                crate::NativeReconciliationActivation,
            )>,
            NativeEvidenceReadError,
        > {
            Err(NativeEvidenceReadError)
        }
        fn read(
            &mut self,
            _: AttemptId,
        ) -> Result<Option<(crate::LoadedAttempt, NativeGuardianBinding)>, NativeEvidenceReadError>
        {
            Ok(None)
        }
        fn sampling_evidence(
            &mut self,
            _: DispatchKey,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeSamplingStatus,
            )>,
            NativeEvidenceReadError,
        > {
            Ok(None)
        }
        fn reconciliation_evidence(
            &mut self,
            _: AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeReconciliationCompletion,
            )>,
            NativeEvidenceReadError,
        > {
            Ok(self.0.take())
        }
    }

    struct ActivationReader(
        Option<(
            crate::LoadedAttempt,
            NativeGuardianBinding,
            crate::NativeReconciliationActivation,
        )>,
    );
    impl NativeEvidenceReader for ActivationReader {
        fn read(
            &mut self,
            _: AttemptId,
        ) -> Result<Option<(crate::LoadedAttempt, NativeGuardianBinding)>, NativeEvidenceReadError>
        {
            Err(NativeEvidenceReadError)
        }
        fn reconciliation_evidence(
            &mut self,
            _: AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeReconciliationCompletion,
            )>,
            NativeEvidenceReadError,
        > {
            Err(NativeEvidenceReadError)
        }
        fn sampling_evidence(
            &mut self,
            _: DispatchKey,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeSamplingStatus,
            )>,
            NativeEvidenceReadError,
        > {
            Err(NativeEvidenceReadError)
        }
        fn activation_evidence(
            &mut self,
            _: AttemptId,
            _: NonZeroU64,
        ) -> Result<
            Option<(
                crate::LoadedAttempt,
                NativeGuardianBinding,
                crate::NativeReconciliationActivation,
            )>,
            NativeEvidenceReadError,
        > {
            Ok(self.0.take())
        }
    }

    #[test]
    fn activation_refuses_missing_staged_capability_fence_and_mismatched_durable_evidence() {
        for change in 0..12 {
            let mut fixture = native_ack_fixture();
            let (session, committed) = commit_sampling_reconciliation(&mut fixture);
            let revision = committed.checkpoint().persistence_revision().unwrap();
            assert!(matches!(
                fixture.server.handle(Request::FinishReconciliation {
                    session,
                    applied_revision: revision.get(),
                    now: 0
                }),
                Response::ReconciliationPrepared { .. }
            ));
            let binding = fixture
                .store
                .load_native_binding(fixture.attempt)
                .unwrap()
                .unwrap();
            let (_, _, completion) = fixture
                .store
                .read_native_reconciliation_evidence(
                    fixture.attempt,
                    binding.current_session().generation,
                )
                .unwrap()
                .unwrap();
            let proof = crate::PreparedNativeReconciliation::after_live_prepare(
                session,
                fixture.attempt,
                binding.identity(),
                binding.deadline(),
                revision,
            );
            let (_, _, committed) = fixture
                .store
                .commit_native_reconciliation_activation_at(
                    &binding,
                    completion,
                    proof,
                    MonotonicTick::native_now().unwrap(),
                )
                .unwrap();
            let (session, attempt, recovery, binding_revision) = committed.into_locator();
            let (loaded, mut binding, activation) = fixture
                .store
                .read_native_reconciliation_activation_evidence(
                    attempt,
                    binding.current_session().generation,
                )
                .unwrap()
                .unwrap();
            match change {
                0 => {
                    fixture.server.prepared_activation.take();
                }
                1 => {
                    fixture.server.fence_connection();
                    fixture.server.handle(Request::Hello {
                        build: test_build(),
                    });
                }
                2 => binding.identity.nonce = GuardianLifetimeNonce::new([9; 16]).unwrap(),
                3 => binding.identity.process = identity(99),
                4 => binding.slots.clear(),
                5 => binding.slots[0].process = identity(99),
                6 => binding.deadline = MonotonicTick::new(1),
                7 => binding.revision = PersistenceRevision::first(),
                8 => binding.dispatch_floor = PersistenceRevision::first(),
                9 => binding.sessions.last_mut().unwrap().state = NativeSessionState::Detached,
                10 => {
                    binding.sessions.last_mut().unwrap().generation = NonZeroU64::new(99).unwrap();
                }
                11 => {}
                _ => unreachable!(),
            }
            fixture
                .server
                .stage_native_reader(Box::new(ActivationReader(if change == 11 {
                    None
                } else {
                    Some((loaded, binding, activation))
                })));
            assert!(
                matches!(
                    fixture.server.handle(Request::ActivateReconciliation {
                        session,
                        attempt: attempt.get(),
                        recovery_revision: recovery.get(),
                        binding_revision: binding_revision.get()
                    }),
                    Response::Refused(_)
                ),
                "case {change}"
            );
            assert_ne!(fixture.server.snapshot().state, WireState::Active);
            assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn native_finish_refuses_missing_mismatched_and_historical_completion_evidence() {
        for change in 0..12 {
            let mut fixture = native_ack_fixture();
            let (session, _committed) = commit_sampling_reconciliation(&mut fixture);
            let (loaded, mut binding, mut completion) = fixture
                .store
                .read_native_reconciliation_evidence(
                    fixture.attempt,
                    NonZeroU64::new(session.generation).unwrap(),
                )
                .unwrap()
                .unwrap();
            let revision = completion.recovery_revision().get();
            match change {
                0 => {
                    completion = crate::NativeReconciliationCompletion::new(
                        AttemptId::new(1),
                        completion.session_generation(),
                        completion.interruption_revision(),
                        completion.recovery_revision(),
                        completion.binding_revision(),
                    );
                }
                1 => {
                    completion = crate::NativeReconciliationCompletion::new(
                        completion.attempt_id(),
                        NonZeroU64::new(1).unwrap(),
                        completion.interruption_revision(),
                        completion.recovery_revision(),
                        completion.binding_revision(),
                    );
                }
                2 => {
                    completion = crate::NativeReconciliationCompletion::new(
                        completion.attempt_id(),
                        completion.session_generation(),
                        PersistenceRevision::first(),
                        completion.recovery_revision(),
                        completion.binding_revision(),
                    );
                }
                3 => {
                    completion = crate::NativeReconciliationCompletion::new(
                        completion.attempt_id(),
                        completion.session_generation(),
                        completion.interruption_revision(),
                        PersistenceRevision::first(),
                        completion.binding_revision(),
                    );
                }
                4 => {
                    completion = crate::NativeReconciliationCompletion::new(
                        completion.attempt_id(),
                        completion.session_generation(),
                        completion.interruption_revision(),
                        completion.recovery_revision(),
                        PersistenceRevision::first(),
                    );
                }
                5 => binding.identity.nonce = GuardianLifetimeNonce::new([7; 16]).unwrap(),
                6 => binding.identity.process = identity(99),
                7 => binding.slots[0].process = identity(99),
                8 => binding.slots.clear(),
                9 => binding.sessions.last_mut().unwrap().state = NativeSessionState::Detached,
                10 => binding.deadline = MonotonicTick::new(1),
                11 => {}
                _ => unreachable!(),
            }
            fixture
                .server
                .stage_native_reader(Box::new(ReconciliationReader(if change == 11 {
                    None
                } else {
                    Some((loaded, binding, completion))
                })));
            assert!(
                matches!(
                    fixture.server.handle(Request::FinishReconciliation {
                        session,
                        applied_revision: revision,
                        now: 0
                    }),
                    Response::Refused(_)
                ),
                "case {change}"
            );
            assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
            assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn native_finish_cannot_use_claimed_wire_time_to_bypass_expired_deadline() {
        let attempt = AttemptId::new(929);
        let deadline = MonotonicTick::new(1);
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let committed = DeploymentAttempt::start(
            attempt,
            deadline,
            ResourcePolicy::new(
                NonZeroUsize::new(2).unwrap(),
                0,
                0,
                CpuContentionBasisPoints::new(0).unwrap(),
                MemoryPressure::Critical,
            ),
            identity(41),
        )
        .persist(&mut store)
        .unwrap();
        let (guardian, old) =
            Guardian::new(attempt, deadline, Observer, vec![identity(42)]).unwrap();
        let mut server = GuardianServer::new(guardian, old, test_build()).unwrap();
        let native = NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([4; 16]).unwrap(),
            process: identity(43),
        };
        server.stage_native_identity(native);
        let binding = store
            .commit_initial_binding(
                attempt,
                deadline,
                PersistenceRevision::first(),
                native,
                &[crate::NativeGuardianSlot {
                    process: identity(42),
                    liveness: crate::NativeSlotLiveness::Running,
                }],
            )
            .unwrap();
        let binding = store
            .commit_detached_binding(&binding, binding.slots())
            .unwrap();
        committed
            .into_parts()
            .0
            .interrupt()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        let binding = store
            .commit_fresh_interruption_binding(
                &binding,
                PersistenceRevision::new(NonZeroU64::new(2).unwrap()),
            )
            .unwrap();
        server.handle(Request::Hello {
            build: test_build(),
        });
        server.handle(Request::Fence {
            session: old.into(),
        });
        // Enter through the trusted in-process begin seam so expiry is tested at finish,
        // not rejected earlier by native begin's independent deadline check.
        server.native_unbound = false;
        server.stage_interruption(binding.current_session().interruption_revision);
        let Response::ReconciliationBegun { session } =
            server.handle(Request::BeginReconciliation {
                instance: server.snapshot().instance,
                inventory: server.snapshot().slots,
                interruption_revision: 2,
            })
        else {
            panic!("trusted begin")
        };
        server.native_unbound = true;
        let loaded = store.load_attempt(attempt).unwrap().unwrap();
        let _committed = store
            .commit_native_sampling_recovery_at(loaded, &binding, crate::MonotonicTick::new(0))
            .unwrap();
        let evidence = store
            .read_native_reconciliation_evidence(attempt, binding.current_session().generation)
            .unwrap();
        let completion = evidence.as_ref().unwrap().2;
        server.stage_native_reader(Box::new(ReconciliationReader(evidence)));
        assert!(matches!(
            server.handle(Request::FinishReconciliation {
                session,
                applied_revision: 3,
                now: 0
            }),
            Response::Refused(_)
        ));
        assert_eq!(server.snapshot().state, WireState::Reconciling);
        let permit = crate::ReconciledSessionPermit::after_committed_reconciliation(
            server.session,
            attempt,
            deadline,
            completion.recovery_revision(),
        );
        let prepared = server
            .guardian
            .prepare_reconciliation(server.session, permit, MonotonicTick::new(0))
            .unwrap();
        server.prepared_activation = Some((completion, prepared));
        let proof = crate::PreparedNativeReconciliation::after_live_prepare(
            session,
            attempt,
            binding.identity(),
            deadline,
            completion.recovery_revision(),
        );
        let (_, _, committed) = store
            .commit_native_reconciliation_activation_at(
                &binding,
                completion,
                proof,
                MonotonicTick::new(0),
            )
            .unwrap();
        let (session, attempt, recovery_revision, binding_revision) = committed.into_locator();
        let activation = store
            .read_native_reconciliation_activation_evidence(
                attempt,
                binding.current_session().generation,
            )
            .unwrap();
        server.stage_native_reader(Box::new(ActivationReader(activation)));
        assert_eq!(
            server.handle(Request::ActivateReconciliation {
                session,
                attempt: attempt.get(),
                recovery_revision: recovery_revision.get(),
                binding_revision: binding_revision.get()
            }),
            Response::Refused(WireError::Guardian(WireGuardianError::DeadlineExpired))
        );
        assert_eq!(server.snapshot().state, WireState::AuthorityLost);
        assert!(server.prepared_activation.is_none());
    }

    #[test]
    fn finish_dto_rejects_unknown_authority_fields() {
        let body = serde_json::json!({ "op": "finish_reconciliation", "data": {
            "session": { "instance": 1, "generation": 2 }, "applied_revision": 3, "now": 0,
            "permit": { "reconciled": true }
        }});
        assert!(serde_json::from_value::<Request>(body).is_err());
        let request = serde_json::json!({"op":"activate_reconciliation", "data": {"session":{"instance":1,"generation":2}, "attempt":1, "recovery_revision":3,"binding_revision":4,"permit":true}});
        assert!(serde_json::from_value::<Request>(request).is_err());
        let response = serde_json::json!({"op":"reconciliation_prepared", "data": {"session":{"instance":1,"generation":2}, "attempt":1, "recovery_revision":3,"permit":true}});
        assert!(serde_json::from_value::<Response>(response).is_err());
    }

    #[test]
    fn native_success_ack_is_correlated_idempotent_and_requires_original_receipt() {
        let mut fixture = native_ack_fixture();
        let active_session = fixture.active_session;
        let key = fixture.key;
        let active_request = || Request::AckAfterAppliedRevision {
            session: active_session,
            key,
            applied_revision: 2,
        };
        assert_eq!(
            fixture.server.handle(active_request()),
            Response::Refused(WireError::MissingProof)
        );
        let status = dispatch_and_apply_native_sample(&mut fixture);
        assert!(matches!(
            status,
            crate::NativeSamplingStatus::AppliedPendingAck { .. }
        ));
        for request in [
            Request::AckAfterAppliedRevision {
                session: fixture.active_session,
                key: fixture.key,
                applied_revision: 3,
            },
            Request::AckAfterAppliedRevision {
                session: fixture.active_session,
                key: WireKey {
                    ordinal: 1,
                    ..fixture.key
                },
                applied_revision: 2,
            },
        ] {
            assert_eq!(
                fixture.server.handle(request),
                Response::Refused(WireError::MissingProof)
            );
        }
        let acked = Response::Acked {
            key: fixture.key,
            applied_revision: 2,
        };
        assert_eq!(fixture.server.handle(active_request()), acked);
        assert_eq!(
            fixture
                .server
                .handle(Request::ObserveRetained { key: fixture.key }),
            Response::Retained(None)
        );
        let recovered = reconnect_ack_session(&mut fixture);
        let retry = Request::AckAfterAppliedRevision {
            session: recovered,
            key: fixture.key,
            applied_revision: 2,
        };
        assert_eq!(fixture.server.handle(retry.clone()), acked);
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .store
                .load_native_sampling_status(fixture.issue.key())
                .unwrap(),
            Some(status)
        );
        replacement_server_refuses_ack(&fixture, retry);
    }

    #[test]
    fn native_sampling_submit_revalidates_durable_issue_before_execution() {
        let (mut server, issue, calls, status) = sampling_fixture();
        let key = WireKey::try_from(issue.key()).unwrap();
        let session = server.snapshot().session;
        let Response::Admitted { admission } = server.handle(Request::DispatchAdmit {
            session,
            key,
            effect: WireEffect::SampleResources {
                sequence: issue.sequence(),
            },
        }) else {
            panic!("admit")
        };
        *status.lock().unwrap() = Some(crate::NativeSamplingStatus::ParkedRetained {
            issue,
            failure: crate::ResourceObservationFailure::Unsupported,
            applying_revision: PersistenceRevision::new(NonZeroU64::new(2).unwrap()),
        });
        let submit = || Request::DispatchSubmit {
            session,
            admission,
            now: 0,
        };
        assert_eq!(
            server.handle(submit()),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(
            server.handle(submit()),
            Response::Refused(WireError::UnknownAdmission)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn native_sampling_wrong_submit_session_consumes_admission_and_fence_clears_it() {
        let (mut server, issue, calls, _status) = sampling_fixture();
        let key = WireKey::try_from(issue.key()).unwrap();
        let session = server.snapshot().session;
        let admit = || Request::DispatchAdmit {
            session,
            key,
            effect: WireEffect::SampleResources {
                sequence: issue.sequence(),
            },
        };
        let Response::Admitted { admission } = server.handle(admit()) else {
            panic!("admit")
        };
        let wrong = WireSession {
            generation: session.generation + 1,
            ..session
        };
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session: wrong,
                admission,
                now: 0
            }),
            Response::Refused(GuardianError::StaleSession.into())
        );
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 0
            }),
            Response::Refused(WireError::UnknownAdmission)
        );
        let Response::Admitted { admission } = server.handle(admit()) else {
            panic!("readmit")
        };
        server.fence_connection();
        assert_eq!(server.native_admissions.len(), 0);
        assert!(matches!(
            server.handle(Request::Hello {
                build: test_build()
            }),
            Response::Hello { .. }
        ));
        assert_eq!(
            server.handle(Request::DispatchSubmit {
                session,
                admission,
                now: 0
            }),
            Response::Refused(WireError::UnknownAdmission)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn native_sampling_refuses_wrong_session_key_effect_and_durable_status() {
        for variant in 0..6 {
            let (mut server, issue, calls, _status) = sampling_fixture();
            let key = WireKey::try_from(issue.key()).unwrap();
            let session = server.snapshot().session;
            let mut request = Request::DispatchAdmit {
                session,
                key,
                effect: WireEffect::SampleResources {
                    sequence: issue.sequence(),
                },
            };
            match variant {
                0 => {
                    if let Request::DispatchAdmit { session, .. } = &mut request {
                        session.generation += 1;
                    }
                }
                1 => {
                    if let Request::DispatchAdmit { key, .. } = &mut request {
                        key.revision += 1;
                    }
                }
                2 => {
                    if let Request::DispatchAdmit { effect, .. } = &mut request {
                        *effect = WireEffect::SampleResources {
                            sequence: issue.sequence() + 1,
                        };
                    }
                }
                3 => {
                    if let Request::DispatchAdmit { effect, .. } = &mut request {
                        *effect = WireEffect::ListChildren {
                            parent: identity(42).into(),
                        };
                    }
                }
                4 => {
                    server.native_identity = None;
                }
                5 => {
                    server.native_reader = None;
                }
                _ => unreachable!(),
            }
            assert!(
                matches!(server.handle(request), Response::Refused(_)),
                "variant {variant}"
            );
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
        let (mut server, issue, calls, _status) = sampling_fixture();
        let key = WireKey::try_from(issue.key()).unwrap();
        let session = server.snapshot().session;
        server.fence_connection();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            server.handle(Request::DispatchAdmit {
                session,
                key,
                effect: WireEffect::SampleResources {
                    sequence: issue.sequence()
                }
            }),
            Response::Refused(WireError::BuildMismatch)
        );
        let (mut server, issue, calls, _status) = sampling_fixture_with_status(true);
        let key = WireKey::try_from(issue.key()).unwrap();
        let session = server.snapshot().session;
        assert_eq!(
            server.handle(Request::DispatchAdmit {
                session,
                key,
                effect: WireEffect::SampleResources {
                    sequence: issue.sequence()
                }
            }),
            Response::Refused(WireError::MissingProof)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    fn identity(pid: u32) -> ProcessIdentity {
        ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(pid).unwrap()),
            BirthToken::new(1, 0).unwrap(),
        )
    }
    fn fixture() -> (
        GuardianServer<Observer>,
        crate::LoadedAttempt,
        NativeGuardianBinding,
        Request,
    ) {
        fixture_with_deadline(u64::MAX)
    }
    fn fixture_with_deadline(
        tick: u64,
    ) -> (
        GuardianServer<Observer>,
        crate::LoadedAttempt,
        NativeGuardianBinding,
        Request,
    ) {
        let attempt = AttemptId::new(918);
        let deadline = MonotonicTick::new(tick);
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(0).unwrap(),
            MemoryPressure::Critical,
        );
        DeploymentAttempt::start(attempt, deadline, policy, identity(41))
            .persist(&mut store)
            .unwrap();
        let (guardian, session) = Guardian::new(
            attempt,
            deadline,
            Observer,
            vec![identity(42), identity(44)],
        )
        .unwrap();
        let mut server = GuardianServer::new(guardian, session, test_build()).unwrap();
        server.stage_native_identity(NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([8; 16]).unwrap(),
            process: identity(43),
        });
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        crate::native_guardian::verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        server.fence_connection();
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("detached")
        };
        crate::native_guardian::verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        let active = store.load_attempt(attempt).unwrap().unwrap();
        let checkpoint = active.prepare_recovery().unwrap();
        let crate::RecoveryDisposition::Checkpoint(transition) = checkpoint else {
            panic!("checkpoint")
        };
        let committed = (*transition).persist(&mut store).unwrap();
        assert_eq!(committed.checkpoint().phase(), Phase::Interrupted);
        let revision = committed.checkpoint().persistence_revision().unwrap();
        let old = store.load_native_binding(attempt).unwrap().unwrap();
        let binding = store
            .commit_fresh_interruption_binding(&old, revision)
            .unwrap();
        let loaded = store.load_attempt(attempt).unwrap().unwrap();
        let request = Request::BeginReconciliation {
            instance: snapshot.instance,
            inventory: snapshot.slots,
            interruption_revision: revision.get(),
        };
        (server, loaded, binding, request)
    }
    fn checked(
        mut server: GuardianServer<Observer>,
        loaded: crate::LoadedAttempt,
        binding: NativeGuardianBinding,
        request: Request,
    ) -> Response {
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        server.handle(request)
    }
    struct Gate {
        identity: ProcessIdentity,
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl NativeInitialGate for Gate {
        fn identity(&self) -> ProcessIdentity {
            self.identity
        }
        fn release(self: Box<Self>) -> io::Result<()> {
            self.released
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn initial_fixture(
        native: bool,
    ) -> (
        GuardianServer<Observer>,
        Request,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        crate::LoadedAttempt,
        NativeGuardianBinding,
    ) {
        let attempt = AttemptId::new(919);
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(std::time::Duration::from_secs(120))
            .unwrap();
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(0).unwrap(),
            MemoryPressure::Critical,
        );
        DeploymentAttempt::start(attempt, deadline, policy, identity(41))
            .persist(&mut store)
            .unwrap();
        let (guardian, session) =
            Guardian::new(attempt, deadline, Observer, vec![identity(42)]).unwrap();
        let mut server = GuardianServer::new(guardian, session, test_build()).unwrap();
        let native_identity = NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([8; 16]).unwrap(),
            process: identity(43),
        };
        server.stage_native_identity(native_identity);
        let Response::Hello { snapshot, .. } = server.handle(Request::Hello {
            build: test_build(),
        }) else {
            panic!("hello")
        };
        crate::native_guardian::verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        let loaded = store.load_attempt(attempt).unwrap().unwrap();
        let binding = store.load_native_binding(attempt).unwrap().unwrap();
        let request = initial_release_request(&snapshot, loaded.persistence_revision()).unwrap();
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if native {
            server.stage_initial_gate(Box::new(Gate {
                identity: identity(42),
                released: released.clone(),
            }));
        }
        (server, request, released, loaded, binding)
    }

    #[test]
    fn native_deadline_uses_monotonic_nanoseconds() {
        let now = MonotonicTick::native_now().unwrap();
        let future = now.checked_add(std::time::Duration::from_secs(1)).unwrap();
        assert!(before_native_deadline(future.get()));
        assert!(!before_native_deadline(now.get()));
    }

    #[test]
    fn initial_release_requires_live_gate_reader_and_exact_evidence() {
        use std::sync::atomic::Ordering;
        let (mut server, request, released, loaded, binding) = initial_fixture(true);
        assert_eq!(
            server.handle(request.clone()),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        let response = server.handle(request.clone());
        assert!(matches!(response, Response::InitialReleased { .. }));
        assert!(released.load(Ordering::SeqCst));
        assert_eq!(
            server.handle(request),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );

        let (mut server, request, released, loaded, binding) = initial_fixture(false);
        assert_eq!(
            server.handle(request.clone()),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        assert_eq!(
            server.handle(request),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
        assert!(!released.load(Ordering::SeqCst));

        let (mut server, request, released, _, _) = initial_fixture(true);
        server.fence_connection();
        assert_eq!(
            server.handle(request),
            Response::Refused(WireError::BuildMismatch)
        );
        assert!(!released.load(Ordering::SeqCst));

        for change in 0..8 {
            let (mut server, mut request, released, loaded, mut binding) = initial_fixture(true);
            match change {
                0 => {
                    if let Request::ReleaseInitial { nonce, .. } = &mut request {
                        *nonce = [9; 16];
                    }
                }
                1 => {
                    if let Request::ReleaseInitial { session, .. } = &mut request {
                        session.generation += 1;
                    }
                }
                2 => binding.identity.process = identity(99),
                3 => binding.slots[0].process = identity(99),
                4 => binding.dispatch_floor = PersistenceRevision::new(NonZeroU64::new(2).unwrap()),
                5 => binding.sessions.last_mut().unwrap().state = NativeSessionState::Detached,
                6 => binding.deadline = MonotonicTick::new(1),
                7 => {
                    if let Request::ReleaseInitial {
                        attempt_revision, ..
                    } = &mut request
                    {
                        *attempt_revision += 1;
                    }
                }
                _ => unreachable!(),
            }
            server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
            assert!(
                matches!(server.handle(request), Response::Refused(_)),
                "case {change}"
            );
            assert!(!released.load(Ordering::SeqCst), "case {change}");
        }
    }

    #[test]
    fn generic_transport_cannot_release_even_with_staged_evidence_and_gate() {
        let (mut server, request, released, loaded, binding) = initial_fixture(true);
        server.native_unbound = false;
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        assert_eq!(
            server.handle(request),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
        assert!(!released.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn release_initial_with_unknown_field_is_malformed_before_execution() {
        let (mut server, request, released, loaded, binding) = initial_fixture(true);
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        let mut body = serde_json::to_value(&request).unwrap();
        body["data"]["unexpected"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&Envelope {
            version: PROTOCOL_VERSION,
            body,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(
            &u32::try_from(bytes.len())
                .expect("fixed test JSON fits u32")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&bytes);
        assert!(matches!(
            read_frame::<Request>(&mut Cursor::new(frame)),
            Err(CodecError::Malformed(_))
        ));
        assert!(!released.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn native_begin_remains_observation_only() {
        let (mut server, loaded, binding, begin) = fixture();
        server.stage_native_reader(Box::new(Reader(Some((loaded, binding)))));
        let Response::ReconciliationBegun { session } = server.handle(begin) else {
            panic!("durable begin must succeed")
        };
        let key = WireKey {
            attempt: 918,
            revision: 2,
            ordinal: 0,
        };
        let forbidden = [
            Request::FinishReconciliation {
                session,
                applied_revision: 2,
                now: 1,
            },
            Request::AckAfterAppliedRevision {
                session,
                key,
                applied_revision: 2,
            },
            Request::ReserveSignalRound {
                session,
                revision: 2,
                count: 1,
            },
            Request::DispatchAdmit {
                session,
                key,
                effect: WireEffect::SampleResources { sequence: 1 },
            },
            Request::DispatchSubmit {
                session,
                admission: 1,
                now: 1,
            },
        ];
        for request in forbidden {
            let expected = match &request {
                Request::DispatchAdmit { .. }
                | Request::AckAfterAppliedRevision { .. }
                | Request::FinishReconciliation { .. }
                | Request::ActivateReconciliation { .. } => WireError::MissingProof,
                Request::DispatchSubmit { .. } => WireError::UnknownAdmission,
                Request::Hello { .. }
                | Request::Snapshot
                | Request::Fence { .. }
                | Request::BeginReconciliation { .. }
                | Request::ReleaseInitial { .. }
                | Request::Inventory
                | Request::ObserveSlot { .. }
                | Request::ObserveRetained { .. }
                | Request::VerifyRecoveredNativeSampling { .. }
                | Request::ReserveSignalRound { .. } => WireError::UnsupportedDurableBinding,
            };
            assert_eq!(server.handle(request), Response::Refused(expected));
        }
    }

    #[test]
    fn native_begin_requires_exact_durable_checkpoint_and_full_binding() {
        let (mut server, _, _, request) = fixture();
        assert_eq!(
            server.handle(request),
            Response::Refused(WireError::UnsupportedDurableBinding)
        );
        let (server, loaded, binding, request) = fixture();
        assert!(matches!(
            checked(server, loaded, binding, request),
            Response::ReconciliationBegun {
                session: WireSession { generation: 2, .. }
            }
        ));
        for change in 0..12 {
            let (server, loaded, mut binding, mut request) = fixture();
            match change {
                0 => binding.identity.nonce = GuardianLifetimeNonce::new([9; 16]).unwrap(),
                1 => binding.identity.process = identity(99),
                2 => binding.deadline = MonotonicTick::new(500),
                3 => binding.slots[0].process = identity(99),
                4 => binding.slots.clear(),
                5 => binding.sessions.last_mut().unwrap().state = NativeSessionState::Detached,
                6 => binding.sessions.last_mut().unwrap().generation = NonZeroU64::new(3).unwrap(),
                7 => binding.dispatch_floor = PersistenceRevision::first(),
                8 => {
                    if let Request::BeginReconciliation {
                        interruption_revision,
                        ..
                    } = &mut request
                    {
                        *interruption_revision += 1;
                    }
                }
                9 => {
                    binding.sessions.last_mut().unwrap().interruption_revision =
                        PersistenceRevision::first();
                }
                10 => {
                    if let Request::BeginReconciliation { inventory, .. } = &mut request {
                        inventory.clear();
                    }
                }
                11 => binding.slots.swap(0, 1),
                _ => unreachable!(),
            }
            assert_eq!(
                checked(server, loaded, binding, request),
                Response::Refused(WireError::MissingProof),
                "case {change}"
            );
        }
        let (server, _, binding, request) = fixture();
        let mut other = ControllerSqliteStore::in_memory().unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(0).unwrap(),
            MemoryPressure::Critical,
        );
        DeploymentAttempt::start(
            AttemptId::new(918),
            MonotonicTick::new(u64::MAX),
            policy,
            identity(41),
        )
        .persist(&mut other)
        .unwrap();
        let active = other.load_attempt(AttemptId::new(918)).unwrap().unwrap();
        assert_eq!(
            checked(server, active, binding, request),
            Response::Refused(WireError::MissingProof)
        );
        let (server, loaded, binding, request) = fixture_with_deadline(1);
        assert_eq!(
            checked(server, loaded, binding, request),
            Response::Refused(WireError::MissingProof)
        );
    }
}
