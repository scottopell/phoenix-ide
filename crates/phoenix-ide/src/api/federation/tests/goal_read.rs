use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use phoenix_core::domain::{
    close::TranscriptConversationId,
    conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalDisposition, GoalHistoryEntry, GoalId,
        GoalMutation, GoalSnapshot, GoalState, GoalText, GoalTimestampUs, GoalVersion,
    },
    instance_identity::{FederationCredentialVerifier, InstanceId},
    product_conversation::ProductConversationId,
};

use super::super::*;

#[test]
fn remote_goal_wait_uses_bounded_whole_second_wire_shape() {
    let wait: RemoteGoalWaitRequest = serde_json::from_value(serde_json::json!({
        "minimum_version": 7,
        "timeout_seconds": 25,
    }))
    .unwrap();
    let request = wait.into_coordinator_request().unwrap();
    assert_eq!(request.minimum_version(), GoalVersion::parse(7).unwrap());
    assert_eq!(request.timeout(), Duration::from_secs(25));
    assert_eq!(
        serde_json::to_value(wait).unwrap(),
        serde_json::json!({"minimum_version": 7, "timeout_seconds": 25})
    );

    for timeout_seconds in [0, 26] {
        let wait: RemoteGoalWaitRequest = serde_json::from_value(serde_json::json!({
            "minimum_version": 7,
            "timeout_seconds": timeout_seconds,
        }))
        .unwrap();
        assert!(wait.into_coordinator_request().is_err());
    }

    assert!(
        serde_json::from_value::<RemoteGoalWaitRequest>(serde_json::json!({
            "minimum_version": 7,
            "timeout_seconds": 1,
            "unexpected": true,
        }))
        .is_err()
    );
}

#[test]
fn remote_goal_response_wait_disposition_must_match_the_request() {
    let destination = InstanceId::new();
    let caller = InstanceId::new();
    let target = ProductConversationId::parse("goal-product").unwrap();
    let transcript = TranscriptConversationId::parse("goal-transcript").unwrap();
    let mut request = RemoteReadGoalRequest {
        destination_instance_id: destination,
        caller_instance_id: caller,
        target: target.clone(),
        after_version: GoalVersion::INITIAL,
        wait: None,
    };
    let mut response = RemoteReadGoalResponse {
        destination_instance_id: destination,
        caller_instance_id: caller,
        source_instance_id: destination,
        target: target.clone(),
        exact_transcript_id: transcript,
        after_version: GoalVersion::INITIAL,
        wait: None,
        current: None,
        history: Vec::new(),
        wait_status: RemoteGoalWaitStatus::Immediate,
    };

    assert!(validate_remote_goal_response(&response, destination, caller, &request).is_ok());
    response.wait_status = RemoteGoalWaitStatus::Satisfied;
    assert!(validate_remote_goal_response(&response, destination, caller, &request).is_err());

    let wait: RemoteGoalWaitRequest = serde_json::from_value(serde_json::json!({
        "minimum_version": 1,
        "timeout_seconds": 1,
    }))
    .unwrap();
    request.wait = Some(wait);
    response.wait = Some(wait);
    response.wait_status = RemoteGoalWaitStatus::TimedOut;
    assert!(validate_remote_goal_response(&response, destination, caller, &request).is_ok());
    response.wait_status = RemoteGoalWaitStatus::Immediate;
    assert!(validate_remote_goal_response(&response, destination, caller, &request).is_err());
}

fn goal_history_entry_with_id(
    owner: &ProductConversationId,
    version: i64,
    goal_id: &str,
) -> GoalHistoryEntry {
    let version = GoalVersion::parse(version).unwrap();
    let goal_id = GoalId::parse(goal_id).unwrap();
    let objective = GoalText::parse("validate remote Goal history").unwrap();
    let action_id = GoalText::parse(format!("goal-history-v{}", version.get())).unwrap();
    let actor = GoalActor::GeneratedHost {
        action_id: action_id.clone(),
    };
    let (change, state) = if version == GoalVersion::parse(1).unwrap() {
        (
            GoalChange::Create {
                goal_id: goal_id.clone(),
                objective: objective.clone(),
            },
            GoalState::Underway,
        )
    } else {
        let reason = GoalText::parse("history validation blocked state").unwrap();
        (
            GoalChange::SetDisposition {
                goal_id: goal_id.clone(),
                disposition: GoalDisposition::Blocked(reason.clone()),
            },
            GoalState::Blocked(reason),
        )
    };
    let snapshot = GoalSnapshot::new(owner.clone(), goal_id, version, objective, state).unwrap();
    GoalHistoryEntry::new(
        snapshot,
        GoalMutation {
            owner: owner.clone(),
            expected_version: GoalVersion::parse(version.get() - 1).unwrap(),
            action_id,
            attribution: GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            },
            change,
        },
        GoalTimestampUs::parse(version.get()).unwrap(),
    )
    .unwrap()
}

fn goal_history_entry(owner: &ProductConversationId, version: i64) -> GoalHistoryEntry {
    goal_history_entry_with_id(owner, version, "goal-history-validation")
}

#[test]
fn remote_goal_response_rejects_impossible_history() {
    let destination = InstanceId::new();
    let caller = InstanceId::new();
    let target = ProductConversationId::parse("goal-history-target").unwrap();
    let request = RemoteReadGoalRequest {
        destination_instance_id: destination,
        caller_instance_id: caller,
        target: target.clone(),
        after_version: GoalVersion::INITIAL,
        wait: None,
    };
    let response = |current: Option<GoalHistoryEntry>, history: Vec<GoalHistoryEntry>| {
        RemoteReadGoalResponse {
            destination_instance_id: destination,
            caller_instance_id: caller,
            source_instance_id: destination,
            target: target.clone(),
            exact_transcript_id: TranscriptConversationId::parse("goal-history-transcript")
                .unwrap(),
            after_version: GoalVersion::INITIAL,
            wait: None,
            current,
            history,
            wait_status: RemoteGoalWaitStatus::Immediate,
        }
    };
    let first = goal_history_entry(&target, 1);
    let second = goal_history_entry(&target, 2);

    assert!(validate_remote_goal_response(
        &response(Some(second.clone()), vec![first.clone(), second.clone()]),
        destination,
        caller,
        &request,
    )
    .is_ok());
    assert!(validate_remote_goal_response(
        &response(
            Some(goal_history_entry(&target, 101)),
            (1..=100)
                .map(|version| goal_history_entry(&target, version))
                .collect(),
        ),
        destination,
        caller,
        &request,
    )
    .is_ok());

    let alternate_second = goal_history_entry_with_id(&target, 2, "alternate-goal");
    let malformed = [
        response(Some(second.clone()), vec![second.clone(), first.clone()]),
        response(Some(first.clone()), vec![first.clone(), first.clone()]),
        response(None, vec![first.clone()]),
        response(Some(first.clone()), vec![first.clone(), second.clone()]),
        response(Some(second.clone()), Vec::new()),
        response(
            Some(goal_history_entry(&target, 3)),
            vec![first.clone(), goal_history_entry(&target, 3)],
        ),
        response(
            Some(goal_history_entry(&target, 101)),
            (1..=99)
                .map(|version| goal_history_entry(&target, version))
                .collect(),
        ),
        response(
            Some(alternate_second.clone()),
            vec![first.clone(), alternate_second.clone()],
        ),
        response(Some(alternate_second), vec![first.clone(), second.clone()]),
        response(
            Some(goal_history_entry(&target, 101)),
            (1..=101)
                .map(|version| goal_history_entry(&target, version))
                .collect(),
        ),
    ];
    for invalid in malformed {
        assert!(matches!(
            validate_remote_goal_response(&invalid, destination, caller, &request),
            Err(FederationClientError::ProvenanceMismatch(_))
        ));
    }
}

fn peer(caller: InstanceId, bearer: &[u8]) -> PeerAuthenticated {
    PeerAuthenticated {
        send_authority: phoenix_db::FederationSendAuthority::new(
            caller,
            FederationCredentialVerifier::from_bearer(bearer),
        ),
        caller_instance_id: caller,
        caller_display_name: "goal reader".to_string(),
    }
}

#[tokio::test]
async fn peer_goal_read_is_immediate_and_rechecks_revoked_authority() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    state
        .db
        .create_conversation(
            "goal-read-target",
            "Goal read target",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let target = ProductConversationId::parse(
        sqlx::query_scalar::<_, String>(
            "SELECT product_conversation_id FROM conversations WHERE id='goal-read-target'",
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap(),
    )
    .unwrap();
    let destination = state.db.instance_id().await.unwrap();
    let caller = InstanceId::new();
    let bearer = b"peer-goal-read-test";
    state
        .db
        .replace_federation_enrollment(
            caller,
            "goal reader",
            &FederationCredentialVerifier::from_bearer(bearer),
        )
        .await
        .unwrap();
    let request = || RemoteReadGoalRequest {
        destination_instance_id: destination,
        caller_instance_id: caller,
        target: target.clone(),
        after_version: GoalVersion::INITIAL,
        wait: None,
    };
    let limits = Arc::new(tokio::sync::Semaphore::new(1));

    let response = read_goal_with_limits(
        peer(caller, bearer),
        state.clone(),
        request(),
        &limits,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), MAX_REMOTE_RESPONSE_BYTES)
        .await
        .unwrap();
    let response: RemoteReadGoalResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(response.destination_instance_id, destination);
    assert_eq!(response.source_instance_id, destination);
    assert_eq!(response.caller_instance_id, caller);
    assert_eq!(response.target, target);
    assert_eq!(
        response.exact_transcript_id,
        TranscriptConversationId::parse("goal-read-target").unwrap()
    );
    assert_eq!(response.after_version, GoalVersion::INITIAL);
    assert_eq!(response.wait, None);
    assert_eq!(response.wait_status, RemoteGoalWaitStatus::Immediate);
    assert_eq!(response.current, None);
    assert!(response.history.is_empty());

    assert!(state.db.revoke_federation_enrollment(caller).await.unwrap());
    let response = read_goal_with_limits(
        peer(caller, bearer),
        state,
        request(),
        &limits,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
