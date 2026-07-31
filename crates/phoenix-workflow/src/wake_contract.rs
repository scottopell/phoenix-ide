use crate::{Generation, Timestamp, Version};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeSubject {
    pub profile_kind: String,
    pub profile_version: u32,
    pub resource_key: Vec<u8>,
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
    pub profile_kind: String,
    pub profile_version: u32,
    pub payload: Vec<u8>,
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
    pub fn into_terminal(self) -> CanonicalTerminal {
        match self {
            Self::Expired { deadline } => CanonicalTerminal::Expired { deadline },
            Self::Cancelled { cause, occurred_at } => {
                CanonicalTerminal::Cancelled { cause, occurred_at }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenWakeLifecycle {
    Observing,
    TerminalProposed(ProposedTerminal),
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
    pub owner: WakeOwner,
    pub subject: WakeSubject,
    pub condition: WakeCondition,
    pub registered_at: Timestamp,
    pub deadline: Timestamp,
    pub lifecycle: WakeLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeState {
    Absent,
    Present(WakeContract),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeCommand {
    Register {
        id: WakeContractId,
        owner: WakeOwner,
        subject: WakeSubject,
        condition: WakeCondition,
        registered_at: Timestamp,
        deadline: Timestamp,
    },
    ObserveTerminal {
        expected_generation: Generation,
        evidence: TerminalEvidence,
    },
    Cancel {
        expected_generation: Generation,
        cause: CancellationCause,
        occurred_at: Timestamp,
    },
    DeadlineElapsed {
        expected_generation: Generation,
        observed_at: Timestamp,
    },
    FinalizeProposedTerminal {
        expected_generation: Generation,
    },
    Forget {
        expected_generation: Generation,
        cause: ForgottenCause,
        occurred_at: Timestamp,
    },
    Fail {
        expected_generation: Generation,
        cause: FailureCause,
        occurred_at: Timestamp,
    },
    TransferOwner {
        expected_generation: Generation,
        new_owner: WakeOwner,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeEventKind {
    Registered,
    OwnerTransferred { previous_owner: WakeOwner },
    TerminalProposed { proposal: ProposedTerminal },
    Terminalized { terminal: CanonicalTerminal },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeContractEvent {
    pub contract_id: WakeContractId,
    pub generation: Generation,
    pub version: Version,
    pub owner: WakeOwner,
    pub kind: WakeEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeConflict {
    ContractAlreadyExists,
    ContractMissing,
    InvalidDeadline,
    StaleGeneration {
        expected: Generation,
        actual: Generation,
    },
    AlreadyClosed,
    DeadlineNotReached,
    TerminalArbitrationPending,
    EvidenceDidNotPrecedeProposal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeDisposition {
    Applied { event: WakeContractEvent },
    Replayed { event: WakeContractEvent },
    Rejected(WakeConflict),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOwedEffect {
    BeginObservation {
        contract_id: WakeContractId,
        generation: Generation,
        subject: WakeSubject,
        condition: WakeCondition,
        deadline: Timestamp,
    },
    StopObservation {
        contract_id: WakeContractId,
        generation: Generation,
    },
    AwaitObservationDrain {
        contract_id: WakeContractId,
        generation: Generation,
    },
    CommitTerminalization {
        event: WakeContractEvent,
    },
    TransferDeliveryOwner {
        contract_id: WakeContractId,
        generation: Generation,
        previous_owner: WakeOwner,
        new_owner: WakeOwner,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTransition {
    pub disposition: WakeDisposition,
    pub new_state: WakeState,
    pub owed_effects: Vec<WakeOwedEffect>,
}

#[must_use]
pub fn transition(state: &WakeState, command: WakeCommand) -> WakeTransition {
    match (state, command) {
        (
            WakeState::Absent,
            WakeCommand::Register {
                id,
                owner,
                subject,
                condition,
                registered_at,
                deadline,
            },
        ) => register(id, owner, subject, condition, registered_at, deadline),
        (WakeState::Absent, _) => rejected(state, WakeConflict::ContractMissing),
        (WakeState::Present(_), WakeCommand::Register { .. }) => {
            rejected(state, WakeConflict::ContractAlreadyExists)
        }
        (WakeState::Present(contract), command) => transition_present(contract, command),
    }
}

fn register(
    id: WakeContractId,
    owner: WakeOwner,
    subject: WakeSubject,
    condition: WakeCondition,
    registered_at: Timestamp,
    deadline: Timestamp,
) -> WakeTransition {
    if deadline < registered_at {
        return rejected(&WakeState::Absent, WakeConflict::InvalidDeadline);
    }
    let contract = WakeContract {
        id: id.clone(),
        generation: Generation(0),
        version: Version(1),
        owner,
        subject: subject.clone(),
        condition: condition.clone(),
        registered_at,
        deadline,
        lifecycle: WakeLifecycle::Open(OpenWakeLifecycle::Observing),
    };
    let event = event(&contract, WakeEventKind::Registered);
    WakeTransition {
        disposition: WakeDisposition::Applied {
            event: event.clone(),
        },
        new_state: WakeState::Present(contract.clone()),
        owed_effects: vec![WakeOwedEffect::BeginObservation {
            contract_id: id,
            generation: contract.generation,
            subject,
            condition,
            deadline,
        }],
    }
}

#[allow(clippy::match_same_arms)]
#[allow(clippy::too_many_lines)]
fn transition_present(contract: &WakeContract, command: WakeCommand) -> WakeTransition {
    let expected_generation = match &command {
        WakeCommand::ObserveTerminal {
            expected_generation,
            ..
        }
        | WakeCommand::Cancel {
            expected_generation,
            ..
        }
        | WakeCommand::DeadlineElapsed {
            expected_generation,
            ..
        }
        | WakeCommand::FinalizeProposedTerminal {
            expected_generation,
        }
        | WakeCommand::Forget {
            expected_generation,
            ..
        }
        | WakeCommand::Fail {
            expected_generation,
            ..
        }
        | WakeCommand::TransferOwner {
            expected_generation,
            ..
        } => *expected_generation,
        WakeCommand::Register { .. } => unreachable!("register handled before present transition"),
    };
    if expected_generation != contract.generation {
        return rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::StaleGeneration {
                expected: expected_generation,
                actual: contract.generation,
            },
        );
    }

    match (&contract.lifecycle, command) {
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommand::ObserveTerminal { evidence, .. },
        ) => close(contract, &CanonicalTerminal::Fired { evidence }),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommand::Cancel {
                cause, occurred_at, ..
            },
        ) => propose(
            contract,
            &ProposedTerminal::Cancelled { cause, occurred_at },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommand::DeadlineElapsed { observed_at, .. },
        ) if observed_at >= contract.deadline => propose(
            contract,
            &ProposedTerminal::Expired {
                deadline: contract.deadline,
            },
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommand::DeadlineElapsed { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::DeadlineNotReached,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal)),
            WakeCommand::ObserveTerminal { evidence, .. },
        ) if evidence.occurred_at < proposal.occurred_at() => {
            close(contract, &CanonicalTerminal::Fired { evidence })
        }
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)),
            WakeCommand::ObserveTerminal { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::EvidenceDidNotPrecedeProposal,
        ),
        (
            WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal)),
            WakeCommand::FinalizeProposedTerminal { .. },
        ) => close(contract, &proposal.clone().into_terminal()),
        (
            WakeLifecycle::Open(_),
            WakeCommand::Forget {
                cause, occurred_at, ..
            },
        ) => close(
            contract,
            &CanonicalTerminal::Forgotten { cause, occurred_at },
        ),
        (
            WakeLifecycle::Open(_),
            WakeCommand::Fail {
                cause, occurred_at, ..
            },
        ) => close(contract, &CanonicalTerminal::Failed { cause, occurred_at }),
        (WakeLifecycle::Open(_), WakeCommand::TransferOwner { new_owner, .. }) => {
            transfer_owner(contract, new_owner)
        }
        (
            WakeLifecycle::Open(OpenWakeLifecycle::Observing),
            WakeCommand::FinalizeProposedTerminal { .. },
        ) => rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::TerminalArbitrationPending,
        ),
        (WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)), _) => rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::TerminalArbitrationPending,
        ),
        (WakeLifecycle::Closed(terminal), WakeCommand::ObserveTerminal { ref evidence, .. })
            if terminal
                == &(CanonicalTerminal::Fired {
                    evidence: evidence.clone(),
                }) =>
        {
            replay_closed(contract)
        }
        (WakeLifecycle::Closed(_), _) => rejected(
            &WakeState::Present(contract.clone()),
            WakeConflict::AlreadyClosed,
        ),
        (_, WakeCommand::Register { .. }) => {
            unreachable!("register handled before present transition")
        }
    }
}

fn close(contract: &WakeContract, terminal: &CanonicalTerminal) -> WakeTransition {
    let mut next = contract.clone();
    next.version = next.version.next();
    next.lifecycle = WakeLifecycle::Closed(terminal.clone());
    let event = event(
        &next,
        WakeEventKind::Terminalized {
            terminal: terminal.clone(),
        },
    );
    WakeTransition {
        disposition: WakeDisposition::Applied {
            event: event.clone(),
        },
        new_state: WakeState::Present(next),
        owed_effects: vec![
            WakeOwedEffect::StopObservation {
                contract_id: contract.id.clone(),
                generation: contract.generation,
            },
            WakeOwedEffect::CommitTerminalization { event },
        ],
    }
}

fn propose(contract: &WakeContract, proposal: &ProposedTerminal) -> WakeTransition {
    let mut next = contract.clone();
    next.version = next.version.next();
    next.lifecycle = WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(proposal.clone()));
    let event = event(
        &next,
        WakeEventKind::TerminalProposed {
            proposal: proposal.clone(),
        },
    );
    WakeTransition {
        disposition: WakeDisposition::Applied { event },
        new_state: WakeState::Present(next),
        owed_effects: vec![
            WakeOwedEffect::StopObservation {
                contract_id: contract.id.clone(),
                generation: contract.generation,
            },
            WakeOwedEffect::AwaitObservationDrain {
                contract_id: contract.id.clone(),
                generation: contract.generation,
            },
        ],
    }
}

fn transfer_owner(contract: &WakeContract, new_owner: WakeOwner) -> WakeTransition {
    if new_owner == contract.owner {
        return WakeTransition {
            disposition: WakeDisposition::Replayed {
                event: event(
                    contract,
                    WakeEventKind::OwnerTransferred {
                        previous_owner: contract.owner.clone(),
                    },
                ),
            },
            new_state: WakeState::Present(contract.clone()),
            owed_effects: vec![],
        };
    }
    let previous_owner = contract.owner.clone();
    let mut next = contract.clone();
    next.version = next.version.next();
    next.owner = new_owner.clone();
    let event = event(
        &next,
        WakeEventKind::OwnerTransferred {
            previous_owner: previous_owner.clone(),
        },
    );
    WakeTransition {
        disposition: WakeDisposition::Applied {
            event: event.clone(),
        },
        new_state: WakeState::Present(next),
        owed_effects: vec![WakeOwedEffect::TransferDeliveryOwner {
            contract_id: contract.id.clone(),
            generation: contract.generation,
            previous_owner,
            new_owner,
        }],
    }
}

fn replay_closed(contract: &WakeContract) -> WakeTransition {
    let WakeLifecycle::Closed(terminal) = &contract.lifecycle else {
        unreachable!("closed replay requires closed lifecycle")
    };
    WakeTransition {
        disposition: WakeDisposition::Replayed {
            event: event(
                contract,
                WakeEventKind::Terminalized {
                    terminal: terminal.clone(),
                },
            ),
        },
        new_state: WakeState::Present(contract.clone()),
        owed_effects: vec![],
    }
}

fn event(contract: &WakeContract, kind: WakeEventKind) -> WakeContractEvent {
    WakeContractEvent {
        contract_id: contract.id.clone(),
        generation: contract.generation,
        version: contract.version,
        owner: contract.owner.clone(),
        kind,
    }
}

fn rejected(state: &WakeState, conflict: WakeConflict) -> WakeTransition {
    WakeTransition {
        disposition: WakeDisposition::Rejected(conflict),
        new_state: state.clone(),
        owed_effects: vec![],
    }
}

#[cfg(test)]
mod proptests;
