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
pub trait NativeEvidenceReader: Send {
    /// # Errors
    /// Returns an error when the pinned store cannot establish strict typed evidence.
    fn read(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(crate::LoadedAttempt, crate::NativeGuardianBinding)>, NativeEvidenceReadError>;
}

/// Process-local, one-use release handle. Durable rows and wire values cannot create one.
pub trait NativeInitialGate: Send {
    fn identity(&self) -> ProcessIdentity;
    /// # Errors
    /// An unsuccessful write never grants permission to retry the consumed gate.
    fn release(self: Box<Self>) -> io::Result<()>;
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

/// A clean, exact source revision; arbitrary strings cannot enter the native protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildIdentity(String);

impl BuildIdentity {
    /// # Errors
    /// Rejects abbreviated, uppercase, and non-hexadecimal revisions.
    pub fn parse(value: &str) -> Result<Self, InvalidBuildIdentity> {
        if value.len() == 40
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidBuildIdentity)
        }
    }

    #[must_use]
    /// No native identity exists for a dirty or unidentifiable build.
    pub fn compiled() -> Option<Self> {
        Self::parse(env!("PHOENIX_SDC_BUILD_ID")).ok()
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

pub const PROTOCOL_VERSION: u16 = 2;
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
#[serde(tag = "op", content = "data", rename_all = "snake_case")]
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
    ReconciliationFinished,
    Inventory(Vec<WireSlot>),
    SlotObserved(WireObservation),
    Retained(Option<WireRetained>),
    Acked,
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

// Affine authority is held server-side; only correlation keys can be decoded.
pub struct GuardianServer<E: GuardianExecutor> {
    guardian: Guardian<E>,
    session: ControllerSession,
    build: BuildIdentity,
    hello_verified: bool,
    interruption: Option<PersistenceRevision>,
    finish_proof: Option<(PersistenceRevision, crate::ReconciledSessionPermit)>,
    ack_proofs: Vec<(DispatchKey, PersistenceRevision, crate::AppliedResultAck)>,
    staged: Vec<DispatchTicket>,
    refused: Vec<DispatchTicket>,
    admissions: BTreeMap<u64, (ControllerSession, DispatchTicket)>,
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
            ack_proofs: Vec::new(),
            staged: Vec::new(),
            refused: Vec::new(),
            admissions: BTreeMap::new(),
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
    pub fn stage_native_reader(&mut self, reader: Box<dyn NativeEvidenceReader>) {
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
        self.interruption = None;
        self.finish_proof = None;
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
                    | Request::Fence { .. }
                    | Request::BeginReconciliation { .. }
                    | Request::ReleaseInitial { .. }
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
            self.finish_proof = None;
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
            Response::Acked
        })
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
                self.interruption = None;
                self.finish_proof = None;
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
            } => self.dispatch_admit(session, key, &effect)?,
            Request::DispatchSubmit {
                session,
                admission,
                now,
            } => self.dispatch_submit(session, admission, now)?,
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
    BuildIdentity::parse("0123456789abcdef0123456789abcdef01234567").unwrap()
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
        let other = BuildIdentity::parse("abcdef0123456789abcdef0123456789abcdef01").unwrap();
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
            "ABCDEF0123456789abcdef0123456789abcdef01",
            "g123456789abcdef0123456789abcdef01234567",
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
        fn read(
            &mut self,
            _: AttemptId,
        ) -> Result<Option<(crate::LoadedAttempt, NativeGuardianBinding)>, NativeEvidenceReadError>
        {
            Ok(self.0.take())
        }
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
            assert_eq!(
                server.handle(request),
                Response::Refused(WireError::UnsupportedDurableBinding)
            );
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
