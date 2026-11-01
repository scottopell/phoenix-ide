use super::*;
use crate::{
    AttemptId, BirthToken, GuardianLifetimeNonce, LoopProgress, NativeGuardianIdentity, Phase,
    PolicyLoop, ProcessId, ProcessIdentity, ResourcePolicy,
};
use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};

const FAILURES: [ResourceObservationFailure; 3] = [
    ResourceObservationFailure::TemporarilyUnavailable,
    ResourceObservationFailure::Unsupported,
    ResourceObservationFailure::Malformed,
];

struct FailureFixture {
    store: ControllerSqliteStore,
    issue: NativeSamplingIssue,
    failure: ResourceObservationFailure,
    applying_revision: PersistenceRevision,
    binding: NativeGuardianBinding,
}

impl FailureFixture {
    fn new(failure: ResourceObservationFailure) -> Self {
        let (mut store, mut policy, binding, issue, ticket) = active_fixture(2);
        assert!(matches!(
            policy
                .apply_native_sampling(
                    VerifiedNativeSamplingCompletion::new(
                        NativeSamplingDispatch::new(ticket, issue),
                        NativeSamplingCompletion::Failure(failure),
                    ),
                    &mut store,
                )
                .unwrap(),
            LoopProgress::Parked { .. }
        ));
        let applying_revision = policy.summary().revision;
        assert_eq!(
            store.load_native_sampling_status(issue.key()).unwrap(),
            Some(NativeSamplingStatus::ParkedRetained {
                issue,
                failure,
                applying_revision
            })
        );
        let detached = store.commit_detached_binding(&binding, &[]).unwrap();
        let interrupted = read_attempt(&store.connection, issue.key().attempt_id())
            .unwrap()
            .unwrap()
            .interrupt()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        let binding = store
            .commit_fresh_interruption_binding(
                &detached,
                interrupted.checkpoint().persistence_revision().unwrap(),
            )
            .unwrap();
        assert_eq!(binding.identity(), issue.guardian_identity());
        assert_eq!(
            binding.current_session().state,
            NativeSessionState::Reconciling
        );
        Self {
            store,
            issue,
            failure,
            applying_revision,
            binding,
        }
    }

    fn checkpoint(&self) -> DeploymentAttempt {
        read_attempt(&self.store.connection, self.issue.key().attempt_id())
            .unwrap()
            .unwrap()
    }

    fn candidate(&mut self) -> NativeSamplingFailureSettlementCandidate {
        let loaded = self
            .store
            .load_attempt(self.issue.key().attempt_id())
            .unwrap()
            .unwrap();
        self.store
            .prepare_native_sampling_failure_settlement_at(
                loaded,
                self.issue.key(),
                MonotonicTick::new(499),
            )
            .unwrap()
    }

    fn settle(&mut self) -> NativeSamplingStatus {
        let candidate = self.candidate();
        self.store
            .settle_native_sampling_failure_at(
                VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
                MonotonicTick::new(499),
            )
            .unwrap()
    }

    fn confirmation(
        &self,
        status: NativeSamplingStatus,
    ) -> VerifiedNativeSamplingFailureAckConfirmation {
        let NativeSamplingStatus::FailureSettledPendingAck {
            settlement_revision,
            ..
        } = status
        else {
            panic!("settled failure")
        };
        VerifiedNativeSamplingFailureAckConfirmation::new(
            self.issue,
            self.failure,
            self.applying_revision,
            settlement_revision,
            self.binding.current_session().generation,
        )
    }

    fn reconnect(&mut self) {
        let detached = self
            .store
            .commit_detached_binding(&self.binding, &[])
            .unwrap();
        let loaded = self
            .store
            .load_attempt(self.issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let refreshed = loaded
            .refresh_interruption()
            .unwrap()
            .persist(&mut self.store)
            .unwrap();
        self.binding = self
            .store
            .commit_fresh_interruption_binding(
                &detached,
                refreshed.checkpoint().persistence_revision().unwrap(),
            )
            .unwrap();
    }

    fn assert_rolled_back(&mut self, before: &DeploymentAttempt) {
        assert_eq!(&self.checkpoint(), before);
        assert_eq!(
            read_status(&self.store.connection, self.issue.key()).unwrap(),
            Some(NativeSamplingStatus::ParkedRetained {
                issue: self.issue,
                failure: self.failure,
                applying_revision: self.applying_revision,
            })
        );
        for table in [
            "native_sampling_failure_settlements",
            "native_sampling_failure_ack_confirmations",
        ] {
            assert_eq!(
                self.store
                    .connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0),)
                    .unwrap(),
                0
            );
        }
        assert!(matches!(
            self.store.load_attempt(self.issue.key().attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}

fn active_fixture(
    required: usize,
) -> (
    ControllerSqliteStore,
    PolicyLoop,
    NativeGuardianBinding,
    NativeSamplingIssue,
    DispatchTicket,
) {
    let mut store = ControllerSqliteStore::in_memory().unwrap();
    let id = AttemptId::new(722);
    let process = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(42).unwrap()),
        BirthToken::new(1, 0).unwrap(),
    );
    let committed = DeploymentAttempt::start(
        id,
        MonotonicTick::new(500),
        ResourcePolicy::new(
            NonZeroUsize::new(required).unwrap(),
            10,
            10,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        ),
        process,
    )
    .persist(&mut store)
    .unwrap();
    let binding = store
        .commit_initial_binding(
            id,
            MonotonicTick::new(500),
            PersistenceRevision::first(),
            NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([7; 16]).unwrap(),
                process,
            },
            &[],
        )
        .unwrap();
    let mut policy = PolicyLoop::from_committed(committed);
    let ticket = policy.issue_next().unwrap();
    let issue = store
        .commit_native_sampling_issue(&ticket, &binding)
        .unwrap();
    (store, policy, binding, issue, ticket)
}

fn unchanged_tables(
    store: &ControllerSqliteStore,
) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let mut names = store.connection.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT IN ('attempts','native_sampling_failure_settlements','native_sampling_failure_ack_confirmations') ORDER BY name",
    ).unwrap();
    let tables = names
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|name| {
            let name = name.unwrap();
            let mut statement = store
                .connection
                .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<Vec<rusqlite::types::Value>>>>()
                .unwrap();
            (name, rows)
        })
        .collect();
    tables
}

#[test]
fn every_failure_settles_once_preserving_interrupted_checkpoint_and_all_other_tables() {
    for failure in FAILURES {
        let mut fixture = FailureFixture::new(failure);
        let mut expected = fixture.checkpoint();
        assert_eq!(expected.phase(), Phase::Interrupted);
        let tables = unchanged_tables(&fixture.store);
        let candidate = fixture.candidate();
        let replay = fixture.candidate();
        let status = fixture
            .store
            .settle_native_sampling_failure_at(
                VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
                MonotonicTick::new(499),
            )
            .unwrap();
        let revision = expected
            .persistence_revision()
            .unwrap()
            .checked_next()
            .unwrap();
        expected.set_revision(revision);
        assert_eq!(fixture.checkpoint(), expected);
        assert_eq!(unchanged_tables(&fixture.store), tables);
        assert_eq!(
            status,
            NativeSamplingStatus::FailureSettledPendingAck {
                issue: fixture.issue,
                failure,
                applying_revision: fixture.applying_revision,
                settlement_revision: revision,
                settling_session_generation: fixture.binding.current_session().generation,
            }
        );
        assert_eq!(
            fixture
                .store
                .load_native_sampling_status(fixture.issue.key())
                .unwrap(),
            Some(status)
        );
        assert!(matches!(
            fixture.store.settle_native_sampling_failure_at(
                VerifiedNativeSamplingFailureSettlement::after_live_verification(replay),
                MonotonicTick::new(499),
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(fixture.checkpoint(), expected);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_failure_settlements",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn failure_ack_is_revision_neutral_immediate_or_fresh_and_remains_historical() {
    for failure in FAILURES {
        for later in [false, true] {
            let mut fixture = FailureFixture::new(failure);
            assert_eq!(
                fixture
                    .store
                    .native_sampling_continuation_status(fixture.issue.key().attempt_id())
                    .unwrap(),
                NativeSamplingContinuationStatus::Unavailable
            );
            let pending = fixture.settle();
            if later {
                fixture.reconnect();
            }
            assert_eq!(
                fixture
                    .store
                    .load_native_sampling_status(fixture.issue.key())
                    .unwrap(),
                Some(pending)
            );
            assert_eq!(
                fixture
                    .store
                    .native_sampling_continuation_status(fixture.issue.key().attempt_id())
                    .unwrap(),
                NativeSamplingContinuationStatus::Unavailable
            );
            let before = fixture.checkpoint();
            let tables = unchanged_tables(&fixture.store);
            let confirmed = fixture
                .store
                .confirm_native_sampling_failure_ack(fixture.confirmation(pending))
                .unwrap();
            let NativeSamplingStatus::FailureSettledPendingAck {
                settlement_revision,
                settling_session_generation,
                ..
            } = pending
            else {
                unreachable!()
            };
            assert_eq!(
                confirmed,
                NativeSamplingStatus::FailureSettlementAckConfirmed {
                    issue: fixture.issue,
                    failure,
                    applying_revision: fixture.applying_revision,
                    settlement_revision,
                    settling_session_generation,
                    confirming_session_generation: fixture.binding.current_session().generation,
                }
            );
            assert_eq!(fixture.checkpoint(), before);
            assert_eq!(unchanged_tables(&fixture.store), tables);
            fixture.reconnect();
            assert_eq!(
                fixture
                    .store
                    .load_native_sampling_status(fixture.issue.key())
                    .unwrap(),
                Some(confirmed)
            );
            assert_eq!(
                fixture
                    .store
                    .native_sampling_continuation_status(fixture.issue.key().attempt_id())
                    .unwrap(),
                NativeSamplingContinuationStatus::Unavailable
            );
            assert_eq!(
                fixture
                    .store
                    .sole_native_sampling_failure(fixture.issue.key().attempt_id())
                    .unwrap(),
                Some((fixture.issue.key(), confirmed))
            );
            assert!(matches!(
                fixture
                    .store
                    .confirm_native_sampling_failure_ack(fixture.confirmation(pending)),
                Err(ControllerStoreError::RevisionConflict)
            ));
        }
    }
}

#[test]
fn settlement_and_confirmed_failure_never_satisfy_settled_samples_barrier() {
    for confirmed in [false, true] {
        let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
        let pending = fixture.settle();
        if confirmed {
            fixture
                .store
                .confirm_native_sampling_failure_ack(fixture.confirmation(pending))
                .unwrap();
        }
        let before = fixture.checkpoint();
        assert!(matches!(
            fixture
                .store
                .preflight_native_sampling_recovery(fixture.issue.key().attempt_id()),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(fixture.checkpoint(), before);
    }
}

#[test]
fn success_including_terminal_success_cannot_be_settled_as_failure() {
    for required in [1, 2] {
        let (mut store, mut policy, binding, issue, ticket) = active_fixture(required);
        policy
            .apply_native_sampling(
                VerifiedNativeSamplingCompletion::new(
                    NativeSamplingDispatch::new(ticket, issue),
                    NativeSamplingCompletion::Sample(ResourceSample {
                        sequence: issue.sequence(),
                        capacity: ResourceCapacity {
                            available_memory_bytes: 100,
                            available_storage_bytes: 100,
                        },
                        contention: ResourceContention {
                            cpu: CpuContentionBasisPoints::new(0).unwrap(),
                            memory_pressure: MemoryPressure::Nominal,
                        },
                    }),
                ),
                &mut store,
            )
            .unwrap();
        if required == 2 {
            let detached = store.commit_detached_binding(&binding, &[]).unwrap();
            let interrupted = read_attempt(&store.connection, issue.key().attempt_id())
                .unwrap()
                .unwrap()
                .interrupt()
                .unwrap()
                .persist(&mut store)
                .unwrap();
            store
                .commit_fresh_interruption_binding(
                    &detached,
                    interrupted.checkpoint().persistence_revision().unwrap(),
                )
                .unwrap();
        } else {
            assert_eq!(policy.summary().phase, Phase::OwnershipDiscovery);
        }
        let loaded = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let before = loaded.checkpoint.clone();
        assert!(matches!(
            store.prepare_native_sampling_failure_settlement_at(
                loaded,
                issue.key(),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(
            read_attempt(&store.connection, issue.key().attempt_id()).unwrap(),
            Some(before)
        );
        assert!(matches!(
            read_status(&store.connection, issue.key()).unwrap(),
            Some(NativeSamplingStatus::AppliedPendingAck { .. })
        ));
    }
}

#[test]
fn failure_settlement_deadline_after_writer_acquisition_and_precommit_rolls_back() {
    for prepare in [false, true] {
        for ticks in [vec![500], vec![501], vec![499, 500]] {
            let mut fixture = FailureFixture::new(ResourceObservationFailure::Malformed);
            let before = fixture.checkpoint();
            let mut ticks = ticks.into_iter();
            let mut clock = || {
                Ok(MonotonicTick::new(
                    ticks.next().expect("unexpected clock read"),
                ))
            };
            let result = if prepare {
                let loaded = fixture
                    .store
                    .load_attempt(fixture.issue.key().attempt_id())
                    .unwrap()
                    .unwrap();
                fixture
                    .store
                    .prepare_native_sampling_failure_settlement_with_clock(
                        loaded,
                        fixture.issue.key(),
                        &mut clock,
                    )
                    .map(|_| ())
            } else {
                let candidate = fixture.candidate();
                fixture
                    .store
                    .settle_native_sampling_failure_with_clock(
                        VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
                        &mut clock,
                    )
                    .map(|_| ())
            };
            assert!(matches!(
                result,
                Err(ControllerStoreError::RevisionConflict)
            ));
            assert_eq!(ticks.next(), None);
            fixture.assert_rolled_back(&before);
        }
    }
}

#[test]
fn preparation_rejects_wrong_key_stale_checkpoint_and_multiple_evidence() {
    for case in 0..4 {
        let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
        let loaded = fixture
            .store
            .load_attempt(fixture.issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let mut dispatch = fixture.issue.key();
        match case {
            0 => {
                dispatch =
                    DispatchKey::from_committed_parts(AttemptId::new(999), dispatch.revision(), 0);
            }
            1 => {
                dispatch = DispatchKey::from_committed_parts(
                    dispatch.attempt_id(),
                    dispatch.revision(),
                    1,
                );
            }
            2 => fixture.reconnect(),
            3 => insert_extra_issue(&fixture),
            _ => unreachable!(),
        }
        let before = fixture.checkpoint();
        assert!(fixture
            .store
            .prepare_native_sampling_failure_settlement_at(
                loaded,
                dispatch,
                MonotonicTick::new(499)
            )
            .is_err());
        fixture.assert_rolled_back(&before);
    }
}

fn insert_extra_issue(fixture: &FailureFixture) {
    fixture.store.connection.execute(
        "INSERT INTO native_sampling_issues (attempt_id,dispatch_revision,ordinal,session_generation,sequence) VALUES (?1,?2,0,?3,?4)",
        params![key(fixture.issue.key().attempt_id()), Word(fixture.binding.current_session().interruption_revision.get()), Word(fixture.binding.current_session().generation.get()), Word(fixture.issue.sequence())],
    ).unwrap();
}

#[test]
fn settlement_rechecks_changed_failure_session_and_multiple_evidence() {
    for case in 0..3 {
        let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
        let candidate = fixture.candidate();
        match case {
            0 => {
                fixture
                    .store
                    .connection
                    .execute(
                        "UPDATE native_sampling_applications SET outcome='malformed'",
                        [],
                    )
                    .unwrap();
                fixture.failure = ResourceObservationFailure::Malformed;
            }
            1 => fixture.reconnect(),
            2 => insert_extra_issue(&fixture),
            _ => unreachable!(),
        }
        let before = fixture.checkpoint();
        assert!(matches!(
            fixture.store.settle_native_sampling_failure_at(
                VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
                MonotonicTick::new(499),
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        fixture.assert_rolled_back(&before);
    }
}

#[test]
fn failure_ack_rejects_wrong_failure_revision_or_stale_session_proof() {
    for case in 0..5 {
        let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
        let pending = fixture.settle();
        let mut proof = fixture.confirmation(pending);
        match case {
            0 => proof.failure = ResourceObservationFailure::Malformed,
            1 => proof.applying_revision = proof.applying_revision.checked_next().unwrap(),
            2 => proof.settlement_revision = proof.settlement_revision.checked_next().unwrap(),
            3 => fixture.reconnect(),
            4 => {
                fixture.binding = fixture
                    .store
                    .commit_detached_binding(&fixture.binding, &[])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = fixture.checkpoint();
        assert!(matches!(
            fixture.store.confirm_native_sampling_failure_ack(proof),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(fixture.checkpoint(), before);
        assert_eq!(
            read_status(&fixture.store.connection, fixture.issue.key()).unwrap(),
            Some(pending)
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_failure_ack_confirmations",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn status_loader_rejects_settlement_that_reuses_issuing_session() {
    let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
    fixture.settle();
    fixture
        .store
        .connection
        .execute_batch("DROP TRIGGER native_sampling_settlement_no_update")
        .unwrap();
    fixture
        .store
        .connection
        .execute(
            "UPDATE native_sampling_failure_settlements SET settling_session_generation=?1",
            [Word(fixture.issue.session_generation().get())],
        )
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .load_native_sampling_status(fixture.issue.key()),
        Err(ControllerStoreError::Malformed(_))
    ));
}

fn insert_reconciliation_completion(
    fixture: &FailureFixture,
    generation: NonZeroU64,
) -> rusqlite::Result<usize> {
    let session = fixture
        .binding
        .sessions()
        .iter()
        .find(|session| session.generation == generation)
        .unwrap();
    fixture.store.connection.execute(
        "INSERT INTO native_reconciliation_completions(attempt_id,session_generation,interruption_revision,recovery_revision,binding_revision) VALUES(?1,?2,?3,?4,?5)",
        rusqlite::params![
            key(fixture.issue.key().attempt_id()),
            Word(generation.get()),
            Word(session.interruption_revision.get()),
            Word(session.interruption_revision.checked_next().unwrap().get()),
            Word(fixture.binding.revision().get()),
        ],
    )
}

#[test]
fn schema_rejects_failure_settlement_or_ack_from_completed_reconciliation_session() {
    let mut before_settlement = FailureFixture::new(ResourceObservationFailure::Unsupported);
    before_settlement
        .store
        .connection
        .execute_batch("DROP TRIGGER native_reconciliation_completion_requires_exact_preparation;")
        .unwrap();
    insert_reconciliation_completion(
        &before_settlement,
        before_settlement.binding.current_session().generation,
    )
    .unwrap();
    let candidate = before_settlement.candidate();
    assert!(matches!(
        before_settlement.store.settle_native_sampling_failure_at(
            VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
            MonotonicTick::new(499),
        ),
        Err(ControllerStoreError::Sqlite(_))
    ));

    let mut before_ack = FailureFixture::new(ResourceObservationFailure::Unsupported);
    let pending = before_ack.settle();
    before_ack.reconnect();
    before_ack
        .store
        .connection
        .execute_batch("DROP TRIGGER native_reconciliation_completion_requires_exact_preparation;")
        .unwrap();
    insert_reconciliation_completion(&before_ack, before_ack.binding.current_session().generation)
        .unwrap();
    let confirmation = before_ack.confirmation(pending);
    assert!(matches!(
        before_ack
            .store
            .confirm_native_sampling_failure_ack(confirmation),
        Err(ControllerStoreError::Sqlite(_))
    ));
}

#[test]
fn status_loader_rejects_completed_settling_or_confirming_session() {
    let mut settled = FailureFixture::new(ResourceObservationFailure::Unsupported);
    let _pending = settled.settle();
    settled
        .store
        .connection
        .execute_batch("DROP TRIGGER native_reconciliation_completion_requires_exact_preparation;")
        .unwrap();
    assert!(insert_reconciliation_completion(
        &settled,
        settled.binding.current_session().generation,
    )
    .is_err());
    settled
        .store
        .connection
        .execute_batch(
            "DROP TRIGGER native_reconciliation_completion_excludes_failure_settlement_sessions;",
        )
        .unwrap();
    insert_reconciliation_completion(&settled, settled.binding.current_session().generation)
        .unwrap();
    assert!(matches!(
        read_status(&settled.store.connection, settled.issue.key()),
        Err(ControllerStoreError::Malformed(_))
    ));

    let mut confirmed = FailureFixture::new(ResourceObservationFailure::Unsupported);
    let pending = confirmed.settle();
    confirmed.reconnect();
    let confirmation = confirmed.confirmation(pending);
    confirmed
        .store
        .confirm_native_sampling_failure_ack(confirmation)
        .unwrap();
    confirmed
        .store
        .connection
        .execute_batch("DROP TRIGGER native_reconciliation_completion_requires_exact_preparation;")
        .unwrap();
    assert!(insert_reconciliation_completion(
        &confirmed,
        confirmed.binding.current_session().generation,
    )
    .is_err());
    confirmed
        .store
        .connection
        .execute_batch(
            "DROP TRIGGER native_reconciliation_completion_excludes_failure_settlement_sessions;",
        )
        .unwrap();
    insert_reconciliation_completion(&confirmed, confirmed.binding.current_session().generation)
        .unwrap();
    assert!(matches!(
        read_status(&confirmed.store.connection, confirmed.issue.key()),
        Err(ControllerStoreError::Malformed(_))
    ));
}

#[test]
fn immediate_ack_rejects_checkpoint_revision_beyond_settlement() {
    let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
    let pending = fixture.settle();
    let proof = fixture.confirmation(pending);
    fixture
        .store
        .load_attempt(fixture.issue.key().attempt_id())
        .unwrap()
        .unwrap()
        .refresh_interruption()
        .unwrap()
        .persist(&mut fixture.store)
        .unwrap();
    let before = fixture.checkpoint();
    assert!(matches!(
        fixture.store.confirm_native_sampling_failure_ack(proof),
        Err(ControllerStoreError::RevisionConflict)
    ));
    assert_eq!(fixture.checkpoint(), before);
    assert_eq!(
        read_status(&fixture.store.connection, fixture.issue.key()).unwrap(),
        Some(pending)
    );
}

#[test]
fn later_ack_and_status_loader_require_interruption_strictly_after_settlement() {
    for load_corrupt_confirmation in [false, true] {
        let mut fixture = FailureFixture::new(ResourceObservationFailure::Unsupported);
        let pending = fixture.settle();
        let detached = fixture
            .store
            .commit_detached_binding(&fixture.binding, &[])
            .unwrap();
        let revision = fixture.checkpoint().persistence_revision().unwrap();
        fixture.binding = fixture
            .store
            .commit_fresh_interruption_binding(&detached, revision)
            .unwrap();
        assert_eq!(
            fixture
                .store
                .load_native_sampling_status(fixture.issue.key())
                .unwrap(),
            Some(pending)
        );
        let proof = fixture.confirmation(pending);
        if load_corrupt_confirmation {
            fixture.store.connection.execute(
                "INSERT INTO native_sampling_failure_ack_confirmations VALUES (?1,?2,?3,?4,?5,?6)",
                params![key(fixture.issue.key().attempt_id()), Word(fixture.issue.key().revision().get()),
                    0, Word(proof.applying_revision.get()), Word(proof.settlement_revision.get()),
                    Word(proof.confirming_session_generation.get())],
            ).unwrap();
            assert!(matches!(
                fixture
                    .store
                    .load_native_sampling_status(fixture.issue.key()),
                Err(ControllerStoreError::Malformed(_))
            ));
        } else {
            assert!(matches!(
                fixture.store.confirm_native_sampling_failure_ack(proof),
                Err(ControllerStoreError::RevisionConflict)
            ));
            assert_eq!(
                read_status(&fixture.store.connection, fixture.issue.key()).unwrap(),
                Some(pending)
            );
        }
    }
}
