use crate::{Generation, Timestamp, TransitionId, Version};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeContractId(pub String);

impl WakeContractId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeOwner(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeProfileKind(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeProfileVersion(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeProfileRef {
    pub kind: WakeProfileKind,
    pub version: WakeProfileVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeCodecFamily(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeCodecVersion(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeCodecRef {
    pub family: WakeCodecFamily,
    pub version: WakeCodecVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakePayload(pub Vec<u8>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodedWakeValue {
    pub codec: WakeCodecRef,
    pub payload: WakePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeSubject {
    pub profile: WakeProfileRef,
    pub resource: EncodedWakeValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeCondition {
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancellationCause {
    UserRequested,
    LifecycleConflict,
    OwnerDestroyed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForgottenCause {
    ResourceLostAfterRestart,
    ResourceDestroyed,
    AdapterLostAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCause {
    AdapterProtocolViolation,
    EvidenceRejected,
    ManualResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEvidence {
    pub occurred_at: Timestamp,
    pub value: EncodedWakeValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalTerminal {
    Fired {
        evidence: TerminalEvidence,
    },
    Expired {
        deadline: Timestamp,
    },
    Cancelled {
        cause: CancellationCause,
        occurred_at: Timestamp,
    },
    Forgotten {
        cause: ForgottenCause,
        occurred_at: Timestamp,
    },
    Failed {
        cause: FailureCause,
        occurred_at: Timestamp,
    },
}

impl CanonicalTerminal {
    #[must_use]
    pub fn occurred_at(&self) -> Timestamp {
        match self {
            Self::Fired { evidence } => evidence.occurred_at,
            Self::Expired { deadline } => *deadline,
            Self::Cancelled { occurred_at, .. }
            | Self::Forgotten { occurred_at, .. }
            | Self::Failed { occurred_at, .. } => *occurred_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposedTerminal {
    Expired {
        deadline: Timestamp,
    },
    Cancelled {
        cause: CancellationCause,
        occurred_at: Timestamp,
    },
}

impl ProposedTerminal {
    #[must_use]
    pub fn occurred_at(&self) -> Timestamp {
        match self {
            Self::Expired { deadline } => *deadline,
            Self::Cancelled { occurred_at, .. } => *occurred_at,
        }
    }

    #[must_use]
    pub fn admits_evidence_at(&self, occurred_at: Timestamp) -> bool {
        match self {
            Self::Expired { deadline } => occurred_at <= *deadline,
            Self::Cancelled {
                occurred_at: cancelled_at,
                ..
            } => occurred_at < *cancelled_at,
        }
    }

    #[must_use]
    pub fn into_terminal(self) -> CanonicalTerminal {
        match self {
            Self::Expired { deadline } => CanonicalTerminal::Expired { deadline },
            Self::Cancelled { cause, occurred_at } => {
                CanonicalTerminal::Cancelled { cause, occurred_at }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WakeHeadToken {
    pub generation: Generation,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProposal {
    pub terminal: ProposedTerminal,
    pub transition_id: TransitionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenWakeLifecycle {
    Observing,
    // Observation authority is fenced while evidence that was already authoritative is reconciled.
    TerminalProposed(TerminalProposal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeLifecycle {
    Open(OpenWakeLifecycle),
    Closed(CanonicalTerminal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeContract {
    pub id: WakeContractId,
    pub generation: Generation,
    pub version: Version,
    pub head_transition_id: TransitionId,
    pub head_command: Box<WakeCommandKind>,
    pub registration_owner: WakeOwner,
    pub delivery_owner: WakeOwner,
    pub subject: WakeSubject,
    pub condition: WakeCondition,
    pub registered_at: Timestamp,
    pub deadline: Timestamp,
    pub lifecycle: WakeLifecycle,
}

impl WakeContract {
    #[must_use]
    pub fn head(&self) -> WakeHeadToken {
        WakeHeadToken {
            generation: self.generation,
            version: self.version,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeState {
    Absent,
    Present(WakeContract),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationFenceProof {
    contract_id: WakeContractId,
    proposed_head: WakeHeadToken,
    proposal_transition_id: TransitionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileObservation {
    ObservationAuthorityFenced(ObservationFenceProof),
    ResourceUnavailable {
        cause: ForgottenCause,
        occurred_at: Timestamp,
    },
    ProtocolFailure {
        cause: FailureCause,
        occurred_at: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeCommandKind {
    Register {
        id: WakeContractId,
        registration_owner: WakeOwner,
        subject: WakeSubject,
        condition: WakeCondition,
        registered_at: Timestamp,
        deadline: Timestamp,
    },
    ObserveTerminal {
        expected_head: WakeHeadToken,
        evidence: TerminalEvidence,
    },
    Cancel {
        expected_head: WakeHeadToken,
        cause: CancellationCause,
        occurred_at: Timestamp,
    },
    DeadlineElapsed {
        expected_head: WakeHeadToken,
        observed_at: Timestamp,
    },
    TransferDeliveryOwner {
        expected_head: WakeHeadToken,
        new_owner: WakeOwner,
    },
    Reconcile {
        expected_head: WakeHeadToken,
        observation: ReconcileObservation,
    },
}

impl WakeCommandKind {
    fn expected_head(&self) -> Option<WakeHeadToken> {
        match self {
            Self::Register { .. } => None,
            Self::ObserveTerminal { expected_head, .. }
            | Self::Cancel { expected_head, .. }
            | Self::DeadlineElapsed { expected_head, .. }
            | Self::TransferDeliveryOwner { expected_head, .. }
            | Self::Reconcile { expected_head, .. } => Some(*expected_head),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeCommand {
    pub transition_id: TransitionId,
    pub kind: WakeCommandKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeResumePolicy {
    RequestWhenIdle,
    SuppressAutomaticResume,
}

impl CanonicalTerminal {
    #[must_use]
    pub fn resume_policy(&self) -> WakeResumePolicy {
        match self {
            Self::Cancelled { .. } => WakeResumePolicy::SuppressAutomaticResume,
            Self::Fired { .. }
            | Self::Expired { .. }
            | Self::Forgotten { .. }
            | Self::Failed { .. } => WakeResumePolicy::RequestWhenIdle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeEventKind {
    Registered {
        registration_owner: WakeOwner,
        subject: WakeSubject,
        condition: WakeCondition,
        registered_at: Timestamp,
        deadline: Timestamp,
    },
    DeliveryOwnerTransferred {
        previous_owner: WakeOwner,
        new_owner: WakeOwner,
    },
    TerminalProposed {
        proposal: ProposedTerminal,
    },
    Terminalized {
        terminal: CanonicalTerminal,
        delivery_owner: WakeOwner,
        resume_policy: WakeResumePolicy,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WakePublicEventType {
    Registered,
    DeliveryOwnerTransferred,
    TerminalProposed,
    Terminalized,
}

pub struct WakePublicEventRegistry;

impl WakePublicEventRegistry {
    pub const ALL: [WakePublicEventType; 4] = [
        WakePublicEventType::Registered,
        WakePublicEventType::DeliveryOwnerTransferred,
        WakePublicEventType::TerminalProposed,
        WakePublicEventType::Terminalized,
    ];

    #[must_use]
    pub fn classify(event: &WakeEventKind) -> WakePublicEventType {
        match event {
            WakeEventKind::Registered { .. } => WakePublicEventType::Registered,
            WakeEventKind::DeliveryOwnerTransferred { .. } => {
                WakePublicEventType::DeliveryOwnerTransferred
            }
            WakeEventKind::TerminalProposed { .. } => WakePublicEventType::TerminalProposed,
            WakeEventKind::Terminalized { .. } => WakePublicEventType::Terminalized,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeContractEvent {
    pub contract_id: WakeContractId,
    pub head: WakeHeadToken,
    pub transition_id: TransitionId,
    pub kind: WakeEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRejection {
    ContractAlreadyExists,
    ContractMissing,
    InvalidDeadline,
    StaleHead {
        expected: WakeHeadToken,
        actual: WakeHeadToken,
    },
    AlreadyClosed,
    DeadlineNotReached,
    EvidenceAfterDeadline,
    TerminalArbitrationPending,
    EvidenceDidNotPrecedeProposal,
    ObservationFenceProofRequired,
    ObservationFenceProofMismatch,
    AlreadyDeliveryOwner,
    ConflictingTransitionReuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeDisposition {
    Applied {
        event: WakeContractEvent,
    },
    Replayed {
        transition_id: TransitionId,
        head: WakeHeadToken,
    },
    Rejected(WakeRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WakeEffectRole {
    BeginObservation,
    FenceObservationAuthority,
    CommitTerminalization,
    TransferDeliveryOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeEffectKey {
    pub contract_id: WakeContractId,
    pub generation: Generation,
    pub transition_id: TransitionId,
    pub role: WakeEffectRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeOwedEffectKind {
    BeginObservation {
        subject: WakeSubject,
        condition: WakeCondition,
        deadline: Timestamp,
    },
    FenceObservationAuthority {
        proof: ObservationFenceProof,
    },
    CommitTerminalization {
        event: WakeContractEvent,
    },
    TransferDeliveryOwner {
        previous_owner: WakeOwner,
        new_owner: WakeOwner,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeOwedEffect {
    pub key: WakeEffectKey,
    pub kind: WakeOwedEffectKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTransition {
    pub disposition: WakeDisposition,
    pub new_state: WakeState,
    pub owed_effects: Vec<WakeOwedEffect>,
}

#[must_use]
pub fn transition(state: &WakeState, command: WakeCommand) -> WakeTransition {
    let command_kind = command.kind.clone();
    match (state, command.kind) {
        (
            WakeState::Absent,
            WakeCommandKind::Register {
                id,
                registration_owner,
                subject,
                condition,
                registered_at,
                deadline,
            },
        ) => register(
            command.transition_id,
            command_kind,
            id,
            registration_owner,
            subject,
            condition,
            registered_at,
            deadline,
        ),
        (WakeState::Absent, _) => rejected(state, WakeRejection::ContractMissing),
        (WakeState::Present(contract), WakeCommandKind::Register { .. })
            if command.transition_id == contract.head_transition_id
                && command_kind == *contract.head_command =>
        {
            replayed(contract)
        }
        (WakeState::Present(contract), WakeCommandKind::Register { .. })
            if command.transition_id == contract.head_transition_id =>
        {
            rejected(state, WakeRejection::ConflictingTransitionReuse)
        }
        (WakeState::Present(_), WakeCommandKind::Register { .. }) => {
            rejected(state, WakeRejection::ContractAlreadyExists)
        }
        (WakeState::Present(contract), kind) => {
            transition_present(contract, command.transition_id, kind)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn register(
    transition_id: TransitionId,
    command: WakeCommandKind,
    id: WakeContractId,
    registration_owner: WakeOwner,
    subject: WakeSubject,
    condition: WakeCondition,
    registered_at: Timestamp,
    deadline: Timestamp,
) -> WakeTransition {
    if deadline < registered_at {
        return rejected(&WakeState::Absent, WakeRejection::InvalidDeadline);
    }
    let contract = WakeContract {
        id,
        generation: Generation(0),
        version: Version(1),
        head_transition_id: transition_id,
        head_command: Box::new(command),
        registration_owner: registration_owner.clone(),
        delivery_owner: registration_owner,
        subject: subject.clone(),
        condition: condition.clone(),
        registered_at,
        deadline,
        lifecycle: WakeLifecycle::Open(OpenWakeLifecycle::Observing),
    };
    let event = event(
        &contract,
        transition_id,
        WakeEventKind::Registered {
            registration_owner: contract.registration_owner.clone(),
            subject: contract.subject.clone(),
            condition: contract.condition.clone(),
            registered_at,
            deadline,
        },
    );
    applied(
        contract.clone(),
        event,
        vec![effect(
            &contract,
            transition_id,
            WakeEffectRole::BeginObservation,
            WakeOwedEffectKind::BeginObservation {
                subject,
                condition,
                deadline,
            },
        )],
    )
}

#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn transition_present(
    contract: &WakeContract,
    transition_id: TransitionId,
    command: WakeCommandKind,
) -> WakeTransition {
    if transition_id == contract.head_transition_id {
        return if command == *contract.head_command {
            replayed(contract)
        } else {
            rejected(
                &WakeState::Present(contract.clone()),
                WakeRejection::ConflictingTransitionReuse,
            )
        };
    }
    let expected_head = command
        .expected_head()
        .expect("register commands are handled before present transitions");
    if expected_head != contract.head() {
        return rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::StaleHead {
                expected: expected_head,
                actual: contract.head(),
            },
        );
    }

    let committed_command = command.clone();
    match (&contract.lifecycle, command) {
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::ObserveTerminal { evidence, .. },
        ) if evidence.occurred_at <= contract.deadline => close(
            contract,
            transition_id,
            committed_command.clone(),
            CanonicalTerminal::Fired { evidence },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::ObserveTerminal { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::EvidenceAfterDeadline,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::Cancel {
                cause, occurred_at, ..
            },
        ) => propose(
            contract,
            transition_id,
            committed_command.clone(),
            ProposedTerminal::Cancelled { cause, occurred_at },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::DeadlineElapsed { observed_at, .. },
        ) if observed_at >= contract.deadline => propose(
            contract,
            transition_id,
            committed_command.clone(),
            ProposedTerminal::Expired {
                deadline: contract.deadline,
            },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::DeadlineElapsed { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::DeadlineNotReached,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal)),
            WakeCommandKind::ObserveTerminal { evidence, .. },
        ) if proposal.terminal.admits_evidence_at(evidence.occurred_at) => close(
            contract,
            transition_id,
            committed_command.clone(),
            CanonicalTerminal::Fired { evidence },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)),
            WakeCommandKind::ObserveTerminal { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::EvidenceDidNotPrecedeProposal,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal)),
            WakeCommandKind::Reconcile {
                observation: ReconcileObservation::ObservationAuthorityFenced(proof),
                ..
            },
        ) if proof == observation_fence_proof(contract, proposal) => close(
            contract,
            transition_id,
            committed_command.clone(),
            proposal.terminal.clone().into_terminal(),
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)),
            WakeCommandKind::Reconcile {
                observation: ReconcileObservation::ObservationAuthorityFenced(_),
                ..
            },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::ObservationFenceProofMismatch,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)),
            WakeCommandKind::Reconcile { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::ObservationFenceProofRequired,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::Reconcile {
                observation: ReconcileObservation::ResourceUnavailable { cause, occurred_at },
                ..
            },
        ) => close(
            contract,
            transition_id,
            committed_command.clone(),
            CanonicalTerminal::Forgotten { cause, occurred_at },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::Reconcile {
                observation: ReconcileObservation::ProtocolFailure { cause, occurred_at },
                ..
            },
        ) => close(
            contract,
            transition_id,
            committed_command.clone(),
            CanonicalTerminal::Failed { cause, occurred_at },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::Reconcile {
                observation: ReconcileObservation::ObservationAuthorityFenced(_),
                ..
            },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::TerminalArbitrationPending,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommandKind::TransferDeliveryOwner { new_owner, .. },
        ) => transfer_delivery_owner(contract, transition_id, committed_command, new_owner),
        (WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)), _) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::TerminalArbitrationPending,
        ),
        (WakeLifecycle::Closed(_), _) => rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::AlreadyClosed,
        ),
        (_, WakeCommandKind::Register { .. }) => {
            unreachable!("register commands are handled before present transitions")
        }
    }
}

fn close(
    contract: &WakeContract,
    transition_id: TransitionId,
    command: WakeCommandKind,
    terminal: CanonicalTerminal,
) -> WakeTransition {
    let mut next = advanced(contract, transition_id, command);
    next.lifecycle = WakeLifecycle::Closed(terminal.clone());
    let resume_policy = terminal.resume_policy();
    let event = event(
        &next,
        transition_id,
        WakeEventKind::Terminalized {
            terminal,
            delivery_owner: next.delivery_owner.clone(),
            resume_policy,
        },
    );
    applied(
        next.clone(),
        event.clone(),
        vec![effect(
            &next,
            transition_id,
            WakeEffectRole::CommitTerminalization,
            WakeOwedEffectKind::CommitTerminalization { event },
        )],
    )
}

fn propose(
    contract: &WakeContract,
    transition_id: TransitionId,
    command: WakeCommandKind,
    proposal: ProposedTerminal,
) -> WakeTransition {
    let mut next = advanced(contract, transition_id, command);
    next.lifecycle = WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(TerminalProposal {
        terminal: proposal.clone(),
        transition_id,
    }));
    let event = event(
        &next,
        transition_id,
        WakeEventKind::TerminalProposed { proposal },
    );
    let WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(terminal_proposal)) =
        &next.lifecycle
    else {
        unreachable!("proposal was just installed")
    };
    let proof = observation_fence_proof(&next, terminal_proposal);
    applied(
        next.clone(),
        event,
        vec![effect(
            &next,
            transition_id,
            WakeEffectRole::FenceObservationAuthority,
            WakeOwedEffectKind::FenceObservationAuthority { proof },
        )],
    )
}

fn transfer_delivery_owner(
    contract: &WakeContract,
    transition_id: TransitionId,
    command: WakeCommandKind,
    new_owner: WakeOwner,
) -> WakeTransition {
    if new_owner == contract.delivery_owner {
        return rejected(
            &WakeState::Present(contract.clone()),
            WakeRejection::AlreadyDeliveryOwner,
        );
    }
    let previous_owner = contract.delivery_owner.clone();
    let mut next = advanced(contract, transition_id, command);
    next.delivery_owner = new_owner.clone();
    let event = event(
        &next,
        transition_id,
        WakeEventKind::DeliveryOwnerTransferred {
            previous_owner: previous_owner.clone(),
            new_owner: new_owner.clone(),
        },
    );
    applied(
        next.clone(),
        event,
        vec![effect(
            &next,
            transition_id,
            WakeEffectRole::TransferDeliveryOwner,
            WakeOwedEffectKind::TransferDeliveryOwner {
                previous_owner,
                new_owner,
            },
        )],
    )
}

fn advanced(
    contract: &WakeContract,
    transition_id: TransitionId,
    command: WakeCommandKind,
) -> WakeContract {
    let mut next = contract.clone();
    next.version = next.version.next();
    next.head_transition_id = transition_id;
    next.head_command = Box::new(command);
    next
}

fn observation_fence_proof(
    contract: &WakeContract,
    proposal: &TerminalProposal,
) -> ObservationFenceProof {
    ObservationFenceProof {
        contract_id: contract.id.clone(),
        proposed_head: contract.head(),
        proposal_transition_id: proposal.transition_id,
    }
}

fn event(
    contract: &WakeContract,
    transition_id: TransitionId,
    kind: WakeEventKind,
) -> WakeContractEvent {
    WakeContractEvent {
        contract_id: contract.id.clone(),
        head: contract.head(),
        transition_id,
        kind,
    }
}

fn effect(
    contract: &WakeContract,
    transition_id: TransitionId,
    role: WakeEffectRole,
    kind: WakeOwedEffectKind,
) -> WakeOwedEffect {
    WakeOwedEffect {
        key: WakeEffectKey {
            contract_id: contract.id.clone(),
            generation: contract.generation,
            transition_id,
            role,
        },
        kind,
    }
}

fn applied(
    contract: WakeContract,
    event: WakeContractEvent,
    owed_effects: Vec<WakeOwedEffect>,
) -> WakeTransition {
    WakeTransition {
        disposition: WakeDisposition::Applied { event },
        new_state: WakeState::Present(contract),
        owed_effects,
    }
}

fn replayed(contract: &WakeContract) -> WakeTransition {
    WakeTransition {
        disposition: WakeDisposition::Replayed {
            transition_id: contract.head_transition_id,
            head: contract.head(),
        },
        new_state: WakeState::Present(contract.clone()),
        owed_effects: vec![],
    }
}

fn rejected(state: &WakeState, rejection: WakeRejection) -> WakeTransition {
    WakeTransition {
        disposition: WakeDisposition::Rejected(rejection),
        new_state: state.clone(),
        owed_effects: vec![],
    }
}

#[cfg(test)]
mod proptests;
