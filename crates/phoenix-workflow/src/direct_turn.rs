use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationAuthority(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientTurnKey(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TurnAuthorityId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalMessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTurn {
    pub fingerprint: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Accepted { disposition: AcceptedDisposition },
    Terminal(TurnTerminal),
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
    Created { turn_id: TurnAuthorityId },
    ExactReplay { turn_id: TurnAuthorityId },
    Materialized { message_id: CanonicalMessageId },
    MaterializationReplay { message_id: CanonicalMessageId },
    Terminal { generation: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnConflict {
    PreparedSemanticsChanged,
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
            if existing.prepared != prepared
                || existing.lifecycle != (TurnLifecycle::Accepted { disposition })
            {
                return Err(TurnConflict::PreparedSemanticsChanged);
            }
            return Ok(TurnStep {
                outcome: TurnOutcome::ExactReplay {
                    turn_id: *existing_id,
                },
                owed_effects: Vec::new(),
            });
        }
        if disposition == AcceptedDisposition::Runtime {
            if let Some(owner) = self.live_owner.get(&conversation) {
                return Err(TurnConflict::ConversationAlreadyOwned { owner: *owner });
            }
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
            outcome: TurnOutcome::Created { turn_id },
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
        if turn.generation != expected_generation {
            return Err(TurnConflict::StaleGeneration {
                actual: turn.generation,
            });
        }
        if !turn.is_live() {
            return Err(TurnConflict::AlreadyTerminal);
        }
        let owned_conversation = turn.owns_conversation();
        turn.generation = turn.generation.saturating_add(1);
        turn.lifecycle = TurnLifecycle::Terminal(terminal);
        if owned_conversation {
            self.live_owner.remove(&turn.conversation);
        }
        Ok(TurnStep {
            outcome: TurnOutcome::Terminal {
                generation: turn.generation,
            },
            owed_effects: vec![
                OwedTurnEffect::InterruptChildEffects {
                    turn_id,
                    generation: turn.generation,
                },
                OwedTurnEffect::ReleaseConversationOwner { turn_id },
            ],
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
        PreparedTurn {
            fingerprint: format!("fp-{seed}"),
            payload: vec![seed],
        }
    }

    #[test]
    fn rehydration_rejects_multiple_live_owners() {
        let make_turn = |id| DurableTurn {
            id: TurnAuthorityId(id),
            conversation: ConversationAuthority("same".into()),
            client_key: ClientTurnKey(format!("key-{id}")),
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
            client_key: ClientTurnKey("same".into()),
            prepared: prepared(1),
            disposition: AcceptedDisposition::Runtime,
        };
        let created = model.apply(command.clone()).unwrap();
        let replayed = model.apply(command).unwrap();
        assert!(matches!(created.outcome, TurnOutcome::Created { .. }));
        assert!(matches!(replayed.outcome, TurnOutcome::ExactReplay { .. }));
        assert!(model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey("same".into()),
                prepared: prepared(2),
                turn_id: TurnAuthorityId(2),
                disposition: AcceptedDisposition::Runtime,
            })
            .is_err());
        assert!(model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("b".into()),
                client_key: ClientTurnKey("same".into()),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(3),
                disposition: AcceptedDisposition::Runtime,
            })
            .is_ok());
    }

    #[test]
    fn terminal_transition_advances_generation_and_releases_owner() {
        let mut model = DurableTurnModel::default();
        let created = model
            .apply(TurnCommand::Accept {
                conversation: ConversationAuthority("a".into()),
                client_key: ClientTurnKey("turn".into()),
                prepared: prepared(1),
                turn_id: TurnAuthorityId(1),
                disposition: AcceptedDisposition::Runtime,
            })
            .unwrap();
        let TurnOutcome::Created { turn_id } = created.outcome else {
            panic!("expected creation")
        };
        let terminal = model
            .apply(TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            })
            .unwrap();
        assert_eq!(terminal.outcome, TurnOutcome::Terminal { generation: 1 });
        assert_eq!(model.live_owner(&ConversationAuthority("a".into())), None);
        assert!(model
            .apply(TurnCommand::Materialize {
                turn_id,
                expected_generation: 0,
                message_id: CanonicalMessageId("late".into()),
            })
            .is_err());
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
                    client_key: ClientTurnKey(format!("k-{key}")),
                    prepared: prepared(key),
                    disposition: if key % 2 == 0 { AcceptedDisposition::Runtime } else { AcceptedDisposition::Steering },
                });
                if let Ok(TurnStep { outcome: TurnOutcome::Created { turn_id }, .. }) = outcome {
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
