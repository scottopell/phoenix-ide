use super::*;
use crate::{
    NativeSamplingCompletion, NativeSamplingDispatch, NativeSamplingStatus,
    ResourceObservationFailure, VerifiedNativeSamplingCompletion,
    VerifiedNativeSamplingFailureAckConfirmation, VerifiedNativeSamplingFailureSettlement,
};
use std::sync::atomic::Ordering;

fn parked_fixture() -> (NativeAckFixture, WireSession) {
    let mut fixture = native_ack_fixture_with_options(false, std::time::Duration::from_secs(120));
    dispatch_native_without_application(&mut fixture);
    fixture
        .policy
        .apply_native_sampling(
            VerifiedNativeSamplingCompletion::new(
                NativeSamplingDispatch::new(fixture.ticket.take().unwrap(), fixture.issue),
                NativeSamplingCompletion::Failure(ResourceObservationFailure::Unsupported),
            ),
            &mut fixture.store,
        )
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .load_native_sampling_status(fixture.issue.key())
            .unwrap(),
        Some(NativeSamplingStatus::ParkedRetained {
            failure: ResourceObservationFailure::Unsupported,
            ..
        })
    ));
    let session = reconnect_ack_session(&mut fixture);
    (fixture, session)
}

fn verify_request(fixture: &NativeAckFixture, session: WireSession) -> Request {
    Request::VerifyNativeSamplingFailureSettlement {
        session,
        key: fixture.key,
        issuing_session_generation: fixture.issue.session_generation().get(),
    }
}

fn ack_request(fixture: &NativeAckFixture, session: WireSession) -> Request {
    Request::AckNativeSamplingFailureSettlement {
        session,
        key: fixture.key,
        issuing_session_generation: fixture.issue.session_generation().get(),
    }
}

fn settle(fixture: &mut NativeAckFixture, session: WireSession) -> NativeSamplingStatus {
    assert_eq!(
        fixture.server.handle(verify_request(fixture, session)),
        Response::VerifiedNativeSamplingFailureSettlement {
            key: fixture.key,
            sequence: fixture.issue.sequence(),
            failure: WireResourceError::Unsupported,
        }
    );
    let loaded = fixture
        .store
        .load_attempt(fixture.attempt)
        .unwrap()
        .unwrap();
    let candidate = fixture
        .store
        .prepare_native_sampling_failure_settlement(loaded, fixture.issue.key())
        .unwrap();
    fixture
        .store
        .settle_native_sampling_failure(
            VerifiedNativeSamplingFailureSettlement::after_live_verification(candidate),
        )
        .unwrap()
}

fn fresh_ack_session(fixture: &mut NativeAckFixture, refresh: bool) -> WireSession {
    fixture.server.fence_connection();
    let Response::Hello { snapshot, .. } = fixture.server.handle(Request::Hello {
        build: test_build(),
    }) else {
        panic!("detached hello")
    };
    crate::native_guardian::verify_native_binding(&mut fixture.store, fixture.attempt, &snapshot)
        .unwrap();
    let loaded = fixture
        .store
        .load_attempt(fixture.attempt)
        .unwrap()
        .unwrap();
    let interruption = if refresh {
        loaded
            .refresh_interruption()
            .unwrap()
            .persist(&mut fixture.store)
            .unwrap()
            .checkpoint()
            .persistence_revision()
            .unwrap()
    } else {
        loaded.persistence_revision()
    };
    let detached = fixture
        .store
        .load_native_binding(fixture.attempt)
        .unwrap()
        .unwrap();
    fixture.binding = fixture
        .store
        .commit_fresh_interruption_binding(&detached, interruption)
        .unwrap();
    let Response::ReconciliationBegun { session } =
        fixture.server.handle(Request::BeginReconciliation {
            instance: snapshot.instance,
            inventory: snapshot.slots,
            interruption_revision: interruption.get(),
        })
    else {
        panic!("fresh reconciliation")
    };
    assert_eq!(
        session.generation,
        fixture.binding.current_session().generation.get()
    );
    session
}

#[test]
fn guardian_verifies_retained_failure_without_executor_reexecution_or_dropping_custody() {
    let (mut fixture, session) = parked_fixture();
    let request = verify_request(&fixture, session);
    let expected = Response::VerifiedNativeSamplingFailureSettlement {
        key: fixture.key,
        sequence: fixture.issue.sequence(),
        failure: WireResourceError::Unsupported,
    };
    for _ in 0..2 {
        assert_eq!(fixture.server.handle(request.clone()), expected);
        assert!(fixture
            .server
            .guardian
            .retained_result(fixture.issue.key())
            .is_some());
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }
    assert_eq!(
        fixture.server.handle(ack_request(&fixture, session)),
        Response::Refused(WireError::MissingProof)
    );
    assert!(fixture
        .server
        .guardian
        .retained_result(fixture.issue.key())
        .is_some());
}

#[test]
fn guardian_failure_ack_immediate_and_later_fresh_sessions_are_ack_only() {
    for later in [false, true] {
        let (mut fixture, mut session) = parked_fixture();
        let pending = settle(&mut fixture, session);
        let NativeSamplingStatus::FailureSettledPendingAck {
            issue,
            failure,
            applying_revision,
            settlement_revision,
            settling_session_generation,
        } = pending
        else {
            panic!("settlement")
        };
        if later {
            session = fresh_ack_session(&mut fixture, true);
        }
        let before = fixture
            .store
            .load_attempt(fixture.attempt)
            .unwrap()
            .unwrap()
            .persistence_revision();
        assert_eq!(
            fixture.server.handle(ack_request(&fixture, session)),
            Response::NativeSamplingFailureSettlementAcked {
                key: fixture.key,
                settlement_revision: settlement_revision.get()
            }
        );
        assert!(fixture
            .server
            .guardian
            .retained_result(issue.key())
            .is_none());
        assert!(fixture.server.native_retained_origins.is_empty());
        assert_eq!(fixture.server.snapshot().state, WireState::Reconciling);
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        let confirmed = fixture
            .store
            .confirm_native_sampling_failure_ack(VerifiedNativeSamplingFailureAckConfirmation::new(
                issue,
                failure,
                applying_revision,
                settlement_revision,
                NonZeroU64::new(session.generation).unwrap(),
            ))
            .unwrap();
        assert_eq!(
            confirmed,
            NativeSamplingStatus::FailureSettlementAckConfirmed {
                issue,
                failure,
                applying_revision,
                settlement_revision,
                settling_session_generation,
                confirming_session_generation: NonZeroU64::new(session.generation).unwrap(),
            }
        );
        assert_eq!(
            fixture
                .store
                .load_attempt(fixture.attempt)
                .unwrap()
                .unwrap()
                .persistence_revision(),
            before
        );
        fresh_ack_session(&mut fixture, true);
        let mut reopened = ControllerSqliteStore::open_existing(&fixture.path).unwrap();
        assert_eq!(
            reopened.load_native_sampling_status(issue.key()).unwrap(),
            Some(confirmed)
        );
        assert!(fixture
            .store
            .preflight_native_sampling_recovery(fixture.attempt)
            .is_err());
    }
}

#[test]
fn lost_failure_ack_response_replays_same_receipt_even_after_fresh_session() {
    let (mut fixture, session) = parked_fixture();
    let pending = settle(&mut fixture, session);
    let NativeSamplingStatus::FailureSettledPendingAck {
        settlement_revision,
        ..
    } = pending
    else {
        unreachable!()
    };
    let expected = Response::NativeSamplingFailureSettlementAcked {
        key: fixture.key,
        settlement_revision: settlement_revision.get(),
    };
    let request = ack_request(&fixture, session);
    assert_eq!(fixture.server.handle(request.clone()), expected);
    assert_eq!(fixture.server.handle(request), expected);
    assert_eq!(fixture.server.native_failure_ack_receipts.len(), 1);
    assert_eq!(
        fixture
            .store
            .load_native_sampling_status(fixture.issue.key())
            .unwrap(),
        Some(pending)
    );
    let session = fresh_ack_session(&mut fixture, true);
    assert_eq!(
        fixture.server.handle(ack_request(&fixture, session)),
        expected
    );
    assert_eq!(fixture.server.native_failure_ack_receipts.len(), 1);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert!(fixture
        .server
        .guardian
        .retained_result(fixture.issue.key())
        .is_none());
    assert_eq!(
        fixture.server.handle(verify_request(&fixture, session)),
        Response::Refused(WireError::MissingProof)
    );
}

#[test]
fn guardian_failure_ack_rejects_revision_gap_or_later_session_without_new_interruption() {
    for later_without_refresh in [false, true] {
        let (mut fixture, mut session) = parked_fixture();
        settle(&mut fixture, session);
        if later_without_refresh {
            session = fresh_ack_session(&mut fixture, false);
        } else {
            let loaded = fixture
                .store
                .load_attempt(fixture.attempt)
                .unwrap()
                .unwrap();
            loaded
                .refresh_interruption()
                .unwrap()
                .persist(&mut fixture.store)
                .unwrap();
        }
        assert_eq!(
            fixture.server.handle(ack_request(&fixture, session)),
            Response::Refused(WireError::MissingProof)
        );
        assert!(fixture
            .server
            .guardian
            .retained_result(fixture.issue.key())
            .is_some());
        assert!(fixture.server.native_failure_ack_receipts.is_empty());
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn guardian_failure_verification_rejects_wrong_origin_key_session_and_failure() {
    for case in 0..5 {
        let (mut fixture, session) = parked_fixture();
        let mut request = verify_request(&fixture, session);
        match case {
            0 => {
                if let Request::VerifyNativeSamplingFailureSettlement {
                    issuing_session_generation,
                    ..
                } = &mut request
                {
                    *issuing_session_generation = session.generation;
                }
            }
            1 => {
                if let Request::VerifyNativeSamplingFailureSettlement { key, .. } = &mut request {
                    key.revision += 1;
                }
            }
            2 => {
                if let Request::VerifyNativeSamplingFailureSettlement { session, .. } = &mut request
                {
                    *session = fixture.active_session;
                }
            }
            3 => {
                rusqlite::Connection::open(&fixture.path)
                    .unwrap()
                    .execute(
                        "UPDATE native_sampling_applications SET outcome='malformed'",
                        [],
                    )
                    .unwrap();
            }
            4 => {
                fixture.server.native_retained_origins.clear();
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            fixture.server.handle(request),
            Response::Refused(_)
        ));
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert!(fixture
            .server
            .guardian
            .retained_result(fixture.issue.key())
            .is_some());
    }
}

#[test]
fn successful_retained_sample_never_verifies_failure_settlement() {
    let mut fixture = native_ack_fixture();
    dispatch_and_apply_native_sample(&mut fixture);
    let session = reconnect_ack_session(&mut fixture);
    assert_eq!(
        fixture.server.handle(verify_request(&fixture, session)),
        Response::Refused(WireError::MissingProof)
    );
    assert_eq!(
        fixture.server.handle(ack_request(&fixture, session)),
        Response::Refused(WireError::MissingProof)
    );
    assert!(fixture
        .server
        .guardian
        .retained_result(fixture.issue.key())
        .is_some());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}
