use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use sha2::Digest as _;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationAuthority(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientTurnKey(String);

impl ClientTurnKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClientTurnKey {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("client turn key must be non-empty")
    }
}

impl TryFrom<&str> for ClientTurnKey {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.to_string()).ok_or("client turn key must be non-empty")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TurnAuthorityId(pub u64);

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct CanonicalMessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTurn {
    target: ConversationAuthority,
    fingerprint: String,
    payload: Vec<u8>,
}

impl PreparedTurn {
    #[must_use]
    pub fn from_exact_payload(target: &ConversationAuthority, payload: Vec<u8>) -> Self {
        let fingerprint = prepared_fingerprint(target, &payload);
        Self {
            target: target.clone(),
            fingerprint,
            payload,
        }
    }

    /// Rehydrates persisted prepared semantics after validating their checksum.
    ///
    /// # Errors
    /// Returns [`TurnConflict::CorruptAggregate`] when the persisted fingerprint
    /// does not match the exact payload bytes.
    pub fn rehydrate(
        target: &ConversationAuthority,
        fingerprint: String,
        payload: Vec<u8>,
    ) -> Result<Self, TurnConflict> {
        if fingerprint != prepared_fingerprint(target, &payload) {
            return Err(TurnConflict::CorruptAggregate(
                "prepared turn fingerprint does not match payload",
            ));
        }
        Ok(Self {
            target: target.clone(),
            fingerprint,
            payload,
        })
    }

    #[must_use]
    pub fn target(&self) -> &ConversationAuthority {
        &self.target
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

fn prepared_fingerprint(target: &ConversationAuthority, payload: &[u8]) -> String {
    let mut exact = Vec::with_capacity(target.0.len() + 1 + payload.len());
    exact.extend_from_slice(target.0.as_bytes());
    exact.push(0);
    exact.extend_from_slice(payload);
    sha256_hex(&exact)
}

fn sha256_hex(payload: &[u8]) -> String {
    sha2::Sha256::digest(payload)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AcceptedDisposition {
    Runtime,
    Steering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTerminal {
    Completed,
    Cancelled,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnLifecycle {
    Accepted {
        disposition: AcceptedDisposition,
    },
    Terminal {
        terminal: TurnTerminal,
        disposition: AcceptedDisposition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Materialization {
    Unmaterialized,
    Materialized { message_id: CanonicalMessageId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableTurn {
    pub id: TurnAuthorityId,
    pub conversation: ConversationAuthority,
    pub client_key: ClientTurnKey,
    pub prepared: PreparedTurn,
    pub generation: u64,
    pub lifecycle: TurnLifecycle,
    pub materialization: Materialization,
}

impl DurableTurn {
    #[must_use]
    pub fn is_live(&self) -> bool {
        matches!(self.lifecycle, TurnLifecycle::Accepted { .. })
    }

    #[must_use]
    pub fn owns_conversation(&self) -> bool {
        matches!(
            self.lifecycle,
            TurnLifecycle::Accepted {
                disposition: AcceptedDisposition::Runtime
            }
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DurableTurnModel {
    turns: BTreeMap<TurnAuthorityId, DurableTurn>,
    by_scoped_key: BTreeMap<(ConversationAuthority, ClientTurnKey), TurnAuthorityId>,
    live_owner: BTreeMap<ConversationAuthority, TurnAuthorityId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnCommand {
    Accept {
        conversation: ConversationAuthority,
        turn_id: TurnAuthorityId,
        client_key: ClientTurnKey,
        prepared: PreparedTurn,
        disposition: AcceptedDisposition,
    },
    Materialize {
        turn_id: TurnAuthorityId,
        expected_generation: u64,
        message_id: CanonicalMessageId,
    },
    Complete {
        turn_id: TurnAuthorityId,
        expected_generation: u64,
    },
    Cancel {
        turn_id: TurnAuthorityId,
        expected_generation: u64,
    },
    Fail {
        turn_id: TurnAuthorityId,
        expected_generation: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Created {
        turn_id: TurnAuthorityId,
        disposition: AcceptedDisposition,
    },
    ExactReplay {
        turn_id: TurnAuthorityId,
        disposition: AcceptedDisposition,
    },
    Materialized {
        message_id: CanonicalMessageId,
    },
    MaterializationReplay {
        message_id: CanonicalMessageId,
    },
    Terminal {
        generation: u64,
        terminal: TurnTerminal,
        disposition: AcceptedDisposition,
    },
    TerminalReplay {
        turn_id: TurnAuthorityId,
        generation: u64,
        terminal: TurnTerminal,
        disposition: AcceptedDisposition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnConflict {
    PreparedSemanticsChanged { authoritative_fingerprint: String },
    ConversationAlreadyOwned { owner: TurnAuthorityId },
    UnknownTurn,
    StaleGeneration { actual: u64 },
    AlreadyTerminal,
    MaterializationIdentityChanged { canonical: CanonicalMessageId },
    CorruptAggregate(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwedTurnEffect {
    RuntimeDelivery {
        turn_id: TurnAuthorityId,
    },
    SteeringQueue {
        turn_id: TurnAuthorityId,
    },
    TranscriptMaterialization {
        turn_id: TurnAuthorityId,
    },
    ReleaseConversationOwner {
        turn_id: TurnAuthorityId,
    },
    InterruptChildEffects {
        turn_id: TurnAuthorityId,
        generation: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStep {
    pub outcome: TurnOutcome,
    pub owed_effects: Vec<OwedTurnEffect>,
}

impl DurableTurnModel {
    /// Rehydrates a persisted conversation aggregate into the pure model.
    ///
    /// # Errors
    ///
    /// Returns an invariant conflict when scoped identities collide, more than one
    /// runtime turn owns a conversation, or a terminal/non-runtime turn claims
    /// ownership.
    pub fn from_turns(turns: impl IntoIterator<Item = DurableTurn>) -> Result<Self, TurnConflict> {
        let mut model = Self::default();
        for turn in turns {
            let scoped_key = (turn.conversation.clone(), turn.client_key.clone());
            if model.by_scoped_key.insert(scoped_key, turn.id).is_some() {
                return Err(TurnConflict::CorruptAggregate(
                    "duplicate scoped client turn identity",
                ));
            }
            if turn.owns_conversation()
                && model
                    .live_owner
                    .insert(turn.conversation.clone(), turn.id)
                    .is_some()
            {
                return Err(TurnConflict::CorruptAggregate(
                    "multiple live owners for one conversation",
                ));
            }
            if model.turns.insert(turn.id, turn).is_some() {
                return Err(TurnConflict::CorruptAggregate(
                    "duplicate turn authority id",
                ));
            }
        }
        if let Some(violation) = model.invariant_violations().into_iter().next() {
            return Err(TurnConflict::CorruptAggregate(violation));
        }
        Ok(model)
    }

    #[must_use]
    pub fn turn(&self, id: &TurnAuthorityId) -> Option<&DurableTurn> {
        self.turns.get(id)
    }

    #[must_use]
    pub fn live_owner(&self, conversation: &ConversationAuthority) -> Option<&TurnAuthorityId> {
        self.live_owner.get(conversation)
    }

    /// Applies one authoritative direct-turn command.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict when the command does not match the current
    /// scoped identity, generation, owner, lifecycle, or materialization.
    pub fn apply(&mut self, command: TurnCommand) -> Result<TurnStep, TurnConflict> {
        match command {
            TurnCommand::Accept {
                turn_id,
                conversation,
                client_key,
                prepared,
                disposition,
            } => self.accept(turn_id, conversation, client_key, prepared, disposition),
            TurnCommand::Materialize {
                turn_id,
                expected_generation,
                message_id,
            } => self.materialize(turn_id, expected_generation, message_id),
            TurnCommand::Complete {
                turn_id,
                expected_generation,
            } => self.terminate(turn_id, expected_generation, TurnTerminal::Completed),
            TurnCommand::Cancel {
                turn_id,
                expected_generation,
            } => self.terminate(turn_id, expected_generation, TurnTerminal::Cancelled),
            TurnCommand::Fail {
                turn_id,
                expected_generation,
                reason,
            } => self.terminate(
                turn_id,
                expected_generation,
                TurnTerminal::Failed { reason },
            ),
        }
    }

    fn accept(
        &mut self,
        turn_id: TurnAuthorityId,
        conversation: ConversationAuthority,
        client_key: ClientTurnKey,
        prepared: PreparedTurn,
        disposition: AcceptedDisposition,
    ) -> Result<TurnStep, TurnConflict> {
        let scoped_key = (conversation.clone(), client_key.clone());
        if let Some(existing_id) = self.by_scoped_key.get(&scoped_key) {
            let existing = &self.turns[existing_id];
            if existing.prepared != prepared {
                return Err(TurnConflict::PreparedSemanticsChanged {
                    authoritative_fingerprint: existing.prepared.fingerprint().to_string(),
                });
            }
            let outcome = match &existing.lifecycle {
                TurnLifecycle::Accepted {
                    disposition: stored,
                } => TurnOutcome::ExactReplay {
                    turn_id: *existing_id,
                    disposition: *stored,
                },
                TurnLifecycle::Terminal {
                    terminal,
                    disposition: stored,
                } => TurnOutcome::TerminalReplay {
                    turn_id: *existing_id,
                    generation: existing.generation,
                    terminal: terminal.clone(),
                    disposition: *stored,
                },
            };
            return Ok(TurnStep {
                outcome,
                owed_effects: Vec::new(),
            });
        }
        match (disposition, self.live_owner.get(&conversation).copied()) {
            (AcceptedDisposition::Runtime, Some(owner)) => {
                return Err(TurnConflict::ConversationAlreadyOwned { owner });
            }
            (AcceptedDisposition::Steering, None) => {
                return Err(TurnConflict::CorruptAggregate(
                    "steering requires an active runtime owner",
                ));
            }
            _ => {}
        }
        if self.turns.contains_key(&turn_id) {
            return Err(TurnConflict::CorruptAggregate(
                "duplicate turn authority id",
            ));
        }
        let turn = DurableTurn {
            id: turn_id,
            conversation: conversation.clone(),
            client_key,
            prepared,
            generation: 0,
            lifecycle: TurnLifecycle::Accepted { disposition },
            materialization: Materialization::Unmaterialized,
        };
        self.by_scoped_key.insert(scoped_key, turn_id);
        if disposition == AcceptedDisposition::Runtime {
            self.live_owner.insert(conversation, turn_id);
        }
        self.turns.insert(turn_id, turn);
        let owed_effect = match disposition {
            AcceptedDisposition::Runtime => OwedTurnEffect::RuntimeDelivery { turn_id },
            AcceptedDisposition::Steering => OwedTurnEffect::SteeringQueue { turn_id },
        };
        Ok(TurnStep {
            outcome: TurnOutcome::Created {
                turn_id,
                disposition,
            },
            owed_effects: vec![owed_effect],
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn materialize(
        &mut self,
        turn_id: TurnAuthorityId,
        expected_generation: u64,
        message_id: CanonicalMessageId,
    ) -> Result<TurnStep, TurnConflict> {
        let turn = self
            .turns
            .get_mut(&turn_id)
            .ok_or(TurnConflict::UnknownTurn)?;
        if let Materialization::Materialized {
            message_id: canonical,
        } = &turn.materialization
        {
            return if canonical == &message_id {
                Ok(TurnStep {
                    outcome: TurnOutcome::MaterializationReplay {
                        message_id: canonical.clone(),
                    },
                    owed_effects: Vec::new(),
                })
            } else {
                Err(TurnConflict::MaterializationIdentityChanged {
                    canonical: canonical.clone(),
                })
            };
        }
        if turn.generation != expected_generation {
            return Err(TurnConflict::StaleGeneration {
                actual: turn.generation,
            });
        }
        if !turn.is_live() {
            return Err(TurnConflict::AlreadyTerminal);
        }
        match &turn.materialization {
            Materialization::Unmaterialized => {
                turn.materialization = Materialization::Materialized {
                    message_id: message_id.clone(),
                };
                Ok(TurnStep {
                    outcome: TurnOutcome::Materialized {
                        message_id: message_id.clone(),
                    },
                    owed_effects: Vec::new(),
                })
            }
            Materialization::Materialized {
                message_id: canonical,
            } if canonical == &message_id => Ok(TurnStep {
                outcome: TurnOutcome::MaterializationReplay {
                    message_id: canonical.clone(),
                },
                owed_effects: Vec::new(),
            }),
            Materialization::Materialized {
                message_id: canonical,
            } => Err(TurnConflict::MaterializationIdentityChanged {
                canonical: canonical.clone(),
            }),
        }
    }

    fn terminate(
        &mut self,
        turn_id: TurnAuthorityId,
        expected_generation: u64,
        terminal: TurnTerminal,
    ) -> Result<TurnStep, TurnConflict> {
        let turn = self
            .turns
            .get_mut(&turn_id)
            .ok_or(TurnConflict::UnknownTurn)?;
        if let TurnLifecycle::Terminal {
            terminal: stored,
            disposition,
        } = &turn.lifecycle
        {
            if turn.generation == expected_generation.saturating_add(1) {
                if stored == &terminal {
                    return Ok(TurnStep {
                        outcome: TurnOutcome::TerminalReplay {
                            turn_id,
                            generation: turn.generation,
                            terminal,
                            disposition: *disposition,
                        },
                        owed_effects: Vec::new(),
                    });
                }
                return Err(TurnConflict::AlreadyTerminal);
            }
            return Err(TurnConflict::StaleGeneration {
                actual: turn.generation,
            });
        }
        if turn.generation != expected_generation {
            return Err(TurnConflict::StaleGeneration {
                actual: turn.generation,
            });
        }
        let TurnLifecycle::Accepted { disposition } = turn.lifecycle else {
            unreachable!("terminal lifecycle returned above")
        };
        let owned_conversation = disposition == AcceptedDisposition::Runtime;
        turn.generation = turn.generation.saturating_add(1);
        turn.lifecycle = TurnLifecycle::Terminal {
            terminal: terminal.clone(),
            disposition,
        };
        if owned_conversation {
            self.live_owner.remove(&turn.conversation);
        }
        let mut owed_effects = vec![OwedTurnEffect::InterruptChildEffects {
            turn_id,
            generation: turn.generation,
        }];
        if owned_conversation {
            owed_effects.push(OwedTurnEffect::ReleaseConversationOwner { turn_id });
        }
        Ok(TurnStep {
            outcome: TurnOutcome::Terminal {
                generation: turn.generation,
                terminal,
                disposition,
            },
            owed_effects,
        })
    }

    #[must_use]
    pub fn invariant_violations(&self) -> BTreeSet<&'static str> {
        let mut violations = BTreeSet::new();
        for (conversation, owner_id) in &self.live_owner {
            let Some(owner) = self.turns.get(owner_id) else {
                violations.insert("live owner references an unknown turn");
                continue;
            };
            if &owner.conversation != conversation || !owner.is_live() {
                violations.insert("live owner does not reference a live turn in its conversation");
            }
        }
        for turn in self.turns.values() {
            let scoped_key = (turn.conversation.clone(), turn.client_key.clone());
            if self.by_scoped_key.get(&scoped_key) != Some(&turn.id) {
                violations.insert("scoped replay index disagrees with authoritative turn");
            }
            if turn.owns_conversation() && self.live_owner.get(&turn.conversation) != Some(&turn.id)
            {
                violations.insert("runtime-owned turn is not discoverable as live owner");
            }
            if !turn.owns_conversation()
                && self.live_owner.get(&turn.conversation) == Some(&turn.id)
            {
                violations.insert("non-owning turn appears in live owner index");
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn prepared(seed: u8) -> PreparedTurn {
        PreparedTurn::from_exact_payload(&ConversationAuthority("conv-a".to_string()), vec![seed])
    }

    #[test]
    fn client_turn_key_rejects_empty_values() {
        assert_eq!(ClientTurnKey::new(""), None);
        assert!(ClientTurnKey::try_from(String::new()).is_err());
        assert_eq!(ClientTurnKey::try_from("abc").unwrap().as_str(), "abc");
    }

    #[test]
    fn prepared_turn_fingerprint_binds_target_and_exact_payload() {
        let prepared = PreparedTurn::from_exact_payload(
            &ConversationAuthority("conv-a".to_string()),
            b"abc".to_vec(),
        );
        assert_ne!(
            prepared.fingerprint(),
            PreparedTurn::from_exact_payload(
                &ConversationAuthority("conv-b".to_string()),
                b"abc".to_vec(),
            )
            .fingerprint()
        );
        assert_eq!(prepared.payload(), b"abc");
        assert!(PreparedTurn::rehydrate(
            &ConversationAuthority("conv-a".into()),
            prepared.fingerprint().to_string(),
            b"abd".to_vec()
        )
        .is_err());
        assert_eq!(
            PreparedTurn::rehydrate(
                &ConversationAuthority("conv-a".into()),
                prepared.fingerprint().to_string(),
                b"abc".to_vec()
            )
            .unwrap(),
            prepared
        );
    }

    #[test]
    fn rehydration_rejects_multiple_live_owners() {
        let make_turn = |id| DurableTurn {
            id: TurnAuthorityId(id),
            conversation: ConversationAuthority("same".into()),
            client_key: ClientTurnKey::new(format!("key-{id}")).unwrap(),
            prepared: prepared(u8::try_from(id).unwrap()),
            generation: 0,
            lifecycle: TurnLifecycle::Accepted {
                disposition: AcceptedDisposition::Runtime,
            },
            materialization: Materialization::Unmaterialized,
        };
        assert!(matches!(
            DurableTurnModel::from_turns([make_turn(4), make_turn(9)]),
            Err(TurnConflict::CorruptAggregate(_))
        ));
    }

    #[test]
    fn exact_replay_is_scoped_and_semantically_immutable() {
        let mut model = DurableTurnModel::default();
        let command = TurnCommand::Accept {
            turn_id: TurnAuthorityId(1),
            conversation: ConversationAuthority("a".into()),
            client_key: ClientTurnKey::new("same").unwrap(),
            prepared: prepared(1),
            disposition: AcceptedDisposition::Runtime,
        };
        let created = model.apply(command.clone()).unwrap();
        let replayed = model.apply(command).unwrap();
        assert!(matches!(
            created.outcome,
            TurnOutcome::Created {
                disposition: AcceptedDisposition::Runtime,
                ..
            }
        ));
        assert!(matches!(
            replayed.outcome,
            TurnOutcome::ExactReplay {
                disposition: AcceptedDisposition::Runtime,
                ..
            }
        ));
        assert!(model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("same").unwrap(),
                prepared: prepared(2),
                turn_id: TurnAuthorityId(2),
                disposition: AcceptedDisposition::Runtime,
            })
            .is_err());
        assert!(model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("b".into()),
                client_key: ClientTurnKey::new("same").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(3),
                disposition: AcceptedDisposition::Runtime,
            })
            .is_ok());
    }

    #[test]
    fn exact_replay_returns_authoritative_terminal_and_disposition() {
        let mut model = DurableTurnModel::default();
        let created = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("turn").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected creation")
        };
        model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        let replay = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("turn").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(99),
                disposition: AcceptedDisposition::Steering,
            })
            .unwrap();
        assert_eq!(
            replay.outcome,
            TurnOutcome::TerminalReplay {
                turn_id,
                generation: 1,
                terminal: TurnTerminal::Cancelled,
                disposition: AcceptedDisposition::Runtime,
            }
        );
    }

    #[test]
    fn terminal_transition_advances_generation_and_releases_owner() {
        let mut model = DurableTurnModel::default();
        let created = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("turn").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected creation")
        };
        let terminal = model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        assert!(matches!(
            terminal.outcome,
            TurnOutcome::Terminal {
                generation: 1,
                terminal: TurnTerminal::Cancelled,
                disposition: AcceptedDisposition::Runtime,
            }
        ));
        assert_eq!(model.live_owner(&ConversationAuthority("a".into())), None);
        assert!(model
            .apply(TurnCommand::Materialize {
                turn_id,
                expected_generation: 0,
                message_id: CanonicalMessageId("late".into()),
            })
            .is_err());
    }

    #[test]
    fn exact_materialization_replays_after_terminal() {
        let mut model = DurableTurnModel::default();
        model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("turn").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        model
            .apply(TurnCommand::Materialize {
                turn_id: TurnAuthorityId(1),
                expected_generation: 0,
                message_id: CanonicalMessageId("message".into()),
            })
            .unwrap();
        model
            .apply(TurnCommand::Complete {
                turn_id: TurnAuthorityId(1),
                expected_generation: 0,
            })
            .unwrap();

        let replay = model
            .apply(TurnCommand::Materialize {
                turn_id: TurnAuthorityId(1),
                expected_generation: 0,
                message_id: CanonicalMessageId("message".into()),
            })
            .unwrap();
        assert!(matches!(
            replay.outcome,
            TurnOutcome::MaterializationReplay { .. }
        ));
    }

    #[test]
    fn terminal_exact_replay_owes_no_effects_and_conflicts_on_different_terminal() {
        let mut model = DurableTurnModel::default();
        let created = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("turn").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected creation")
        };
        model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        let replay = model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        assert_eq!(
            replay,
            TurnStep {
                outcome: TurnOutcome::TerminalReplay {
                    turn_id,
                    generation: 1,
                    terminal: TurnTerminal::Cancelled,
                    disposition: AcceptedDisposition::Runtime,
                },
                owed_effects: Vec::new(),
            }
        );
        assert_eq!(
            model.apply(TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            }),
            Err(TurnConflict::AlreadyTerminal)
        );
    }

    #[test]
    fn steering_terminal_does_not_release_conversation_owner() {
        let mut model = DurableTurnModel::default();
        model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("runtime").unwrap(),
                prepared: prepared(0),
                turn_id: TurnAuthorityId(9),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        let created = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey::new("steering").unwrap(),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Steering,
            })
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = created.outcome else {
            panic!("expected creation")
        };
        let step = model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        assert_eq!(
            step.owed_effects,
            vec![OwedTurnEffect::InterruptChildEffects {
                turn_id,
                generation: 1
            }]
        );
    }

    proptest! {
        #[test]
        fn generated_histories_preserve_authority_invariants(commands in prop::collection::vec((0u8..4, 0u8..4, any::<bool>()), 0..128)) {
            let mut model = DurableTurnModel::default();
            let mut known = Vec::new();
            for (conversation, key, cancel) in commands {
                let turn_id = TurnAuthorityId(u64::from(conversation) * 16 + u64::from(key) + 1);
                let conversation = ConversationAuthority(format!("c-{conversation}"));
                let outcome = model.apply(TurnCommand::Accept {
                    turn_id,
                    conversation,
                    client_key: ClientTurnKey::new(format!("k-{key}")).unwrap(),
                    prepared: prepared(key),
                    disposition: if key % 2 == 0 { AcceptedDisposition::Runtime } else { AcceptedDisposition::Steering },
                });
                if let Ok(TurnStep { outcome: TurnOutcome::Created { turn_id, .. }, .. }) = outcome {
                    known.push(turn_id);
                    if cancel {
                        let _ = model.apply(TurnCommand::Cancel { turn_id, expected_generation: 0 });
                    }
                }
                prop_assert!(model.invariant_violations().is_empty());
            }
        }
    }
}
