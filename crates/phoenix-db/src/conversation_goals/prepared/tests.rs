use super::*;
use crate::conversation_goals::user_commands::{
    GoalUserRefusal, GoalUserResult, RemoteGoalApplyOutcome,
};
use crate::conversation_goals::writer_tests::{at, counts, fixture, ordinary};

#[test]
fn remote_preparation_expiry_uses_exact_microsecond_boundary() {
    let expiry = 1_700_000_000_123_456;
    assert!(remote_preparation_expired(expiry, expiry));
    assert!(remote_preparation_expired(expiry, expiry + 1));
    assert!(!remote_preparation_expired(expiry, expiry - 1));
}
use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
use phoenix_core::domain::instance_identity::FederationCredentialVerifier;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_event::{
    CancelCause, DirectTurnTurnId, PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
    SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ToolCall, ToolInput};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
};

fn runtime_input(transcript: &str, key: &str) -> crate::workflow::AcceptAuthoritativeTurn {
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: InputOrigin::UserApi,
            text: "source input".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "source input".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    crate::workflow::AcceptAuthoritativeTurn {
        client_key: ClientTurnKey::new(key).unwrap(),
        prepared: PreparedTurn::from_exact_payload(
            &ConversationAuthority(transcript.into()),
            payload.to_exact_bytes().unwrap(),
        ),
        disposition: AcceptedDisposition::Runtime,
        accepted_at: Timestamp(1),
    }
}

async fn coordinator_source(
    db: &Database,
    key: &str,
) -> (
    String,
    ProductConversationId,
    SourceToolCall,
    DirectTurnTurnId,
) {
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let caller = coordinator.id;
    let owner = coordinator.product_conversation_id;
    let accepted = db
        .workflow_repository()
        .accept_authoritative_turn(&runtime_input(&caller, key))
        .await
        .established()
        .expect("Goal preparation fixture turn admission must have an established durable fact")
        .unwrap();
    let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("expected fresh coordinator Runtime turn");
    };
    let source_call = SourceToolCall {
        message_id: format!("{key}-agent"),
        tool_use_id: format!("{key}-call"),
    };
    db.add_message(
        &source_call.message_id,
        &caller,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: source_call.tool_use_id.clone(),
            name: "goal_create".into(),
            input: serde_json::json!({"objective": "from coordinator"}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    (caller, owner, source_call, DirectTurnTurnId(turn_id.0))
}

fn source_tool(id: &str, name: &str) -> ToolCall {
    ToolCall::new(
        id,
        ToolInput::Unknown {
            name: name.into(),
            input: serde_json::json!({"operation": "create"}),
        },
    )
}

fn source_assistant(
    source_call: &SourceToolCall,
    message_id: &str,
    name: &str,
) -> AssistantMessage {
    AssistantMessage::new(
        message_id.into(),
        vec![ContentBlock::tool_use(
            &source_call.tool_use_id,
            name,
            serde_json::json!({"operation": "create"}),
        )],
        None,
        None,
    )
}

fn executing_source_state(
    current_tool: ToolCall,
    remaining_tools: Vec<ToolCall>,
    assistant_message_id: &str,
    assistant_tool_id: &str,
    assistant_tool_name: &str,
) -> crate::ConvState {
    crate::ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            assistant_message_id.into(),
            vec![ContentBlock::tool_use(
                assistant_tool_id,
                assistant_tool_name,
                serde_json::json!({"operation": "create"}),
            )],
            None,
            None,
        ),
    }
}

async fn remove_source_message(db: &Database, caller: &str, source_call: &SourceToolCall) {
    sqlx::query("DELETE FROM messages WHERE conversation_id=? AND message_id=?")
        .bind(caller)
        .bind(&source_call.message_id)
        .execute(db.pool())
        .await
        .unwrap();
}

async fn assert_in_flight_source_refused(
    db: &Database,
    caller: &str,
    owner: &ProductConversationId,
    source_call: &SourceToolCall,
    turn: DirectTurnTurnId,
    state: &crate::ConvState,
) {
    db.update_conversation_state(caller, state).await.unwrap();
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            caller,
            owner,
            source_call,
            turn,
            create_intent("refused source"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(preparation_count(db).await, 0);
}

fn create_intent(objective: &str) -> GoalPreparationIntent {
    GoalPreparationIntent::Create {
        objective: GoalText::parse(objective).unwrap(),
    }
}

async fn preparation_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_preparations")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

async fn remote_source(
    db: &Database,
    key: &str,
) -> (
    crate::FederationSendAuthority,
    phoenix_core::domain::conversation_goal::RemoteGoalAgentSource,
    ProductConversationId,
) {
    use phoenix_core::domain::conversation_goal::{
        GoalAgentSource, GoalExecution, RemoteGoalAgentSource,
    };
    use phoenix_core::domain::instance_identity::{FederationCredentialVerifier, InstanceId};

    let (transcript, source_owner, source_call, turn) = coordinator_source(db, key).await;
    let turn_row_id = i64::try_from(turn.0).unwrap();
    let owner = ordinary(db, &format!("{key}-owner")).await.1;
    let instance_id = InstanceId::new();
    let verifier = FederationCredentialVerifier::from_bearer(key.as_bytes());
    db.replace_federation_enrollment(instance_id, &"remote goal peer".parse().unwrap(), &verifier)
        .await
        .unwrap();
    let authority = crate::FederationSendAuthority::new(instance_id, verifier);
    let workflow_id: i64 =
        sqlx::query_scalar("SELECT workflow_id FROM durable_turns WHERE turn_id=?")
            .bind(turn_row_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let source = RemoteGoalAgentSource {
        instance_id,
        source: GoalAgentSource {
            product_conversation_id: source_owner,
            transcript_id: GoalText::parse(transcript).unwrap(),
            execution: GoalExecution::parse(workflow_id, turn_row_id).unwrap(),
            message_id: GoalText::parse(source_call.message_id).unwrap(),
            tool_use_id: GoalText::parse(source_call.tool_use_id).unwrap(),
        },
    };
    (authority, source, owner)
}

#[tokio::test]
async fn local_busy_refusals_use_live_transcript_and_replay_after_continuation() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "local-busy-live").await;
    let (sampled, owner) = ordinary(&db, "local-busy-live-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("local busy objective"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    db.update_conversation_state(
        &sampled,
        &crate::ConvState::ContextExhausted {
            summary: "continue before local busy refusal".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        db.continue_conversation(&sampled).await.unwrap()
    else {
        panic!("expected a successor transcript");
    };
    assert_ne!(successor.id, sampled);
    let mut expected_counts = counts(&db).await;
    expected_counts.6 += 1;
    let owner_receipt = db
        .reject_busy_user_goal_command(
            &command,
            &sampled,
            || Ok(at()),
            || panic!("an owner busy refusal retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let coordinator_receipt = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &sampled,
            true,
            || Ok(at()),
            || panic!("a Coordinator busy refusal retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    for receipt in [&owner_receipt, &coordinator_receipt] {
        assert!(matches!(
            &receipt.result,
            GoalUserResult::Rejected {
                reason: GoalUserRefusal::Busy,
                ..
            }
        ));
    }
    assert_eq!(
        db.apply_user_goal_command(
            &command,
            &sampled,
            || panic!("an owner busy receipt must replay"),
            || panic!("an owner replay retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        owner_receipt
    );
    assert_eq!(
        db.apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &sampled,
            false,
            || panic!("a Coordinator busy receipt must replay"),
            || panic!("a Coordinator replay retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        coordinator_receipt
    );
    assert_eq!(counts(&db).await, expected_counts);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
}

#[tokio::test]
async fn disabled_auto_continue_remains_an_ordinary_error_for_local_commands() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "local-disabled-create").await;
    let (transcript, owner) = ordinary(&db, "local-disabled-create-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("local disabled create"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    let before = counts(&db).await;
    let owner_result = db
        .apply_user_goal_command(
            &command,
            &transcript,
            || Ok(at()),
            || panic!("a local rollback retains authority"),
        )
        .await
        .established()
        .unwrap();
    let coordinator_result = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("a local Coordinator rollback retains authority"),
        )
        .await
        .established()
        .unwrap();
    for result in [owner_result, coordinator_result] {
        assert!(matches!(result, Err(GoalDbError::AutoContinueDisabled)));
    }
    assert_eq!(counts(&db).await, before);
    assert!(db
        .retained_coordinator_goal_command(&caller, &command)
        .await
        .unwrap()
        .is_none());
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
}

#[tokio::test]
async fn remote_preparation_replay_is_idempotent_and_changed_payload_conflicts() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-preparation-replay").await;
    let first = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("remote objective"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let expires_at_us: i64 = sqlx::query_scalar(
        "SELECT expires_at_us FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(first.reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_micros();
    assert!((now + 899_000_000..=now + 901_000_000).contains(&expires_at_us));
    let replay = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("remote objective"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(replay, first);
    assert!(matches!(
        db.prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("changed objective")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(preparation_count(&db).await, 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn remote_preparation_reference_is_scoped_to_current_enrollment_peer() {
    use phoenix_core::domain::instance_identity::{FederationCredentialVerifier, InstanceId};

    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-preparation-scope").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("scoped objective"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let first_receipt = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(first_receipt) = first_receipt else {
        panic!("newly admitted action must return its receipt");
    };
    assert!(matches!(
        first_receipt.result,
        GoalUserResult::Accepted { .. }
    ));
    let other_id = InstanceId::new();
    let other_verifier = FederationCredentialVerifier::from_bearer(b"other remote peer");
    db.replace_federation_enrollment(
        other_id,
        &"other remote peer".parse().unwrap(),
        &other_verifier,
    )
    .await
    .unwrap();
    let other = crate::FederationSendAuthority::new(other_id, other_verifier);
    assert!(db
        .read_remote_coordinator_goal_preparation(&other, prepared.reference)
        .await
        .is_err());

    let replacement = FederationCredentialVerifier::from_bearer(b"replacement credential");
    db.replace_federation_enrollment(
        source.instance_id,
        &"replacement peer".parse().unwrap(),
        &replacement,
    )
    .await
    .unwrap();
    let replaced = crate::FederationSendAuthority::new(source.instance_id, replacement);
    assert!(db
        .read_remote_coordinator_goal_preparation(&replaced, prepared.reference)
        .await
        .is_err());
    assert!(db
        .apply_remote_coordinator_goal_command(
            &replaced,
            prepared.reference,
            false,
            || panic!("replacement must not apply old preparation"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .is_err());
    let replacement_preparation = db
        .prepare_remote_coordinator_goal_action(
            &replaced,
            &source,
            &owner,
            GoalPreparationIntent::State {
                disposition:
                    phoenix_core::domain::conversation_goal::GoalCommandDisposition::Complete(
                        GoalText::parse("done").unwrap(),
                    ),
            },
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_ne!(replacement_preparation.reference, prepared.reference);
    let enrollment_ids: Vec<String> = sqlx::query_scalar(
        "SELECT enrollment_id FROM conversation_goal_preparations WHERE reference IN (?, ?) ORDER BY reference",
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .bind(replacement_preparation.reference.as_bytes().as_slice())
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(enrollment_ids.len(), 2);
    assert_ne!(enrollment_ids[0], enrollment_ids[1]);
    assert_eq!(preparation_count(&db).await, 2);
    let second_receipt = db
        .apply_remote_coordinator_goal_command(
            &replaced,
            replacement_preparation.reference,
            false,
            || Ok(at()),
            || panic!("replacement enrollment should admit the same source tuple"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(second_receipt) = second_receipt else {
        panic!("newly admitted action must return its receipt");
    };
    assert!(matches!(
        second_receipt.result,
        GoalUserResult::Accepted { .. }
    ));
    assert_eq!(preparation_count(&db).await, 2);
    db.revoke_federation_enrollment(source.instance_id)
        .await
        .unwrap();
    assert!(db
        .read_remote_coordinator_goal_preparation(&replaced, replacement_preparation.reference)
        .await
        .is_err());
}

#[tokio::test]
async fn remote_preparation_lookup_checks_authority_before_classifying_unknown_reference() {
    let db = fixture().await;
    let (authority, source, _) = remote_source(&db, "remote-unknown-authority").await;
    let unknown_reference = Uuid::new_v4();

    assert!(matches!(
        db.read_remote_coordinator_goal_preparation(&authority, unknown_reference)
            .await,
        Err(GoalDbError::TargetChanged)
    ));

    assert!(db
        .revoke_federation_enrollment(source.instance_id)
        .await
        .unwrap());
    assert!(matches!(
        db.read_remote_coordinator_goal_preparation(&authority, unknown_reference)
            .await,
        Err(GoalDbError::Admission(
            crate::DbError::FederationSendAuthorityInactive
        ))
    ));
}

#[tokio::test]
async fn expired_remote_preparation_cannot_be_applied_before_a_receipt() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-expiry").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("expiry"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let expired = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversation_goal_preparations(
           reference,owner_id,source_kind,enrollment_id,expires_at_us,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
           operation,expected_version,observed_goal_id,objective,disposition,reason)
         SELECT ?,owner_id,source_kind,enrollment_id,0,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'expired-call',
           operation,expected_version,observed_goal_id,objective,disposition,reason
         FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(expired.as_bytes().as_slice())
    .bind(prepared.reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let result = db
        .apply_remote_coordinator_goal_command(
            &authority,
            expired,
            false,
            || panic!("expired preparation must not invoke clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(
        result,
        RemoteGoalApplyOutcome::Expired {
            action_reference: expired,
        }
    );
    let apply_state: String = sqlx::query_scalar(
        "SELECT apply_state FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(expired.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(apply_state, "prepared");
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts WHERE preparation_reference=?",
    )
    .bind(expired.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 0);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    assert!(db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn expired_remote_preparation_replays_retained_receipt() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-expired-receipt-replay").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("expired retained refusal"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let expired = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversation_goal_preparations(
           reference,owner_id,source_kind,enrollment_id,expires_at_us,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
           operation,expected_version,observed_goal_id,objective,disposition,reason,apply_state)
         SELECT ?,owner_id,source_kind,enrollment_id,0,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'expired-receipt-call',
           operation,expected_version,observed_goal_id,objective,disposition,reason,'claimed'
         FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(expired.as_bytes().as_slice())
    .bind(prepared.reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let recorded_at = at();
    sqlx::query(
        "INSERT INTO conversation_goal_coordinator_receipts(
           preparation_reference,owner_id,outcome,result_version,recorded_at_us)
         VALUES (?,?,'rejected_busy',0,?)",
    )
    .bind(expired.as_bytes().as_slice())
    .bind(owner.as_str())
    .bind(recorded_at.get())
    .execute(db.pool())
    .await
    .unwrap();

    let replay = db
        .apply_remote_coordinator_goal_command(
            &authority,
            expired,
            false,
            || panic!("retained receipt must be replayed before expiry admission"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(replay) = replay else {
        panic!("retained receipt must replay after expiry");
    };
    assert_eq!(replay.request_id, expired);
    assert_eq!(
        replay.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Busy,
            observed_version: GoalVersion::INITIAL,
            recorded_at,
        }
    );
    for classified in [
        db.claim_remote_coordinator_goal_preparation(
            &authority,
            expired,
            RemoteGoalClaimCommitCut::None,
        )
        .await,
        db.classify_remote_coordinator_goal_claim(
            &authority,
            expired,
            GoalDbError::Invalid("injected lost claim commit acknowledgement"),
        )
        .await,
    ] {
        match classified.unwrap() {
            RemoteCoordinatorGoalClaim::Replay(receipt) => assert_eq!(receipt, replay),
            RemoteCoordinatorGoalClaim::Claimed(_)
            | RemoteCoordinatorGoalClaim::Expired { .. }
            | RemoteCoordinatorGoalClaim::Unclassified => {
                panic!("retained receipt must win over a claimed or expired preparation")
            }
        }
    }
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert!(history.is_empty());
}

async fn remote_unclassified_claim_closes_receiptless_retry(
    claim_commit: RemoteGoalClaimCommitCut,
) {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-claimed-unclassified").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("claimed without receipt"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();

    let before = counts(&db).await;
    let first_authority_losses = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first_authority_losses_for_callback = std::sync::Arc::clone(&first_authority_losses);
    let first = db
        .apply_remote_coordinator_goal_command_with_claim_commit(
            &authority,
            prepared.reference,
            false,
            || {
                assert!(matches!(claim_commit, RemoteGoalClaimCommitCut::None));
                Err(GoalDbError::Invalid("injected post-claim clock failure"))
            },
            move || {
                first_authority_losses_for_callback
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
            claim_commit,
            None,
        )
        .await;
    assert!(matches!(
        first,
        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(
        first_authority_losses.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let apply_state: String = sqlx::query_scalar(
        "SELECT apply_state FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(apply_state, "claimed");
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts WHERE preparation_reference=?",
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 0);

    let retry_authority_losses = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let retry_authority_losses_for_callback = std::sync::Arc::clone(&retry_authority_losses);
    let retry = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || panic!("claimed preparation must not execute again"),
            move || {
                retry_authority_losses_for_callback
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .await;
    assert!(matches!(
        retry,
        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(
        retry_authority_losses.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        first_authority_losses.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(counts(&db).await, before);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[allow(clippy::too_many_lines)]
async fn remote_claimed_action_survives_enrollment_change(replace: bool) {
    use crate::workflow::LocalAuthorityResult;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    let db = fixture().await;
    let (authority, source, owner) = remote_source(
        &db,
        if replace {
            "remote-claim-replaced"
        } else {
            "remote-claim-revoked"
        },
    )
    .await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("admitted before enrollment change"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let claimed = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let pause = RemoteGoalClaimPause {
        claimed: Arc::clone(&claimed),
        resume: Arc::clone(&resume),
    };
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_for_apply = Arc::clone(&callback_count);
    let apply_db = db.clone();
    let apply_authority = authority.clone();
    let reference = prepared.reference;
    let apply = tokio::spawn(async move {
        apply_db
            .apply_remote_coordinator_goal_command_with_claim_commit(
                &apply_authority,
                reference,
                false,
                || Ok(at()),
                move || {
                    callback_count_for_apply.fetch_add(1, Ordering::SeqCst);
                },
                RemoteGoalClaimCommitCut::None,
                Some(pause),
            )
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(30), claimed.notified())
        .await
        .expect("remote Goal claim did not commit within 30 seconds");
    let new_authority = if replace {
        let verifier = FederationCredentialVerifier::from_bearer(b"replacement-credential");
        db.replace_federation_enrollment(
            source.instance_id,
            &"replacement remote goal peer".parse().unwrap(),
            &verifier,
        )
        .await
        .unwrap();
        Some(crate::FederationSendAuthority::new(
            source.instance_id,
            verifier,
        ))
    } else {
        assert!(db
            .revoke_federation_enrollment(source.instance_id)
            .await
            .unwrap());
        None
    };
    resume.notify_one();

    let result = apply.await.unwrap();
    let LocalAuthorityResult::DurableFactEstablished(Ok(receipt)) = result else {
        panic!("admitted Goal action must finish after enrollment changes: {result:?}");
    };
    let RemoteGoalApplyOutcome::Receipt(receipt) = receipt else {
        panic!("claimed action must return its receipt");
    };
    assert_eq!(receipt.request_id, reference);
    assert_eq!(callback_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts WHERE preparation_reference=?",
        )
        .bind(reference.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert!(db.conversation_goal(&owner).await.unwrap().is_some());

    let stale_retry = db
        .apply_remote_coordinator_goal_command(
            &authority,
            reference,
            false,
            || panic!("a retry with stale authority must be denied"),
            || panic!("a denied retry is classified without authority-loss callback"),
        )
        .await;
    assert!(matches!(
        stale_retry,
        LocalAuthorityResult::DurableFactEstablished(Err(GoalDbError::Admission(
            crate::DbError::FederationSendAuthorityInactive
        )))
    ));
    if let Some(new_authority) = new_authority {
        let replacement_retry = db
            .apply_remote_coordinator_goal_command(
                &new_authority,
                reference,
                false,
                || panic!("replacement authority cannot claim an old-enrollment action"),
                || panic!("a denied retry is classified without authority-loss callback"),
            )
            .await;
        assert!(matches!(
            replacement_retry,
            LocalAuthorityResult::DurableFactEstablished(Err(GoalDbError::TargetChanged))
        ));
    }
}

#[tokio::test]
async fn remote_admitted_apply_survives_enrollment_revocation() {
    remote_claimed_action_survives_enrollment_change(false).await;
}

#[tokio::test]
async fn remote_admitted_apply_survives_enrollment_replacement() {
    remote_claimed_action_survives_enrollment_change(true).await;
}

struct TargetHistoryRaceFixture {
    db: Database,
    authority: crate::FederationSendAuthority,
    owner: ProductConversationId,
    reference: Uuid,
    history_before: i64,
}

async fn target_history_race_fixture() -> TargetHistoryRaceFixture {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-target-history-race").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("target closes after claim"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let transcript: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id=? AND runtime_role='user' AND parent_conversation_id IS NULL",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let intervention = crate::conversation_goals::user_commands::GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: crate::conversation_goals::user_commands::GoalUserChange::Create {
            objective: GoalText::parse("intervening objective").unwrap(),
        },
    };
    let intervention_receipt = db
        .apply_user_goal_command(
            &intervention,
            &transcript,
            || Ok(at()),
            || panic!("the intervening owner write must retain authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(
        intervention_receipt.result,
        GoalUserResult::Accepted { .. }
    ));
    assert!(
        db.conversation_goal(&owner)
            .await
            .unwrap()
            .unwrap()
            .version()
            > GoalVersion::INITIAL
    );
    let history_before = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    TargetHistoryRaceFixture {
        db,
        authority,
        owner,
        reference: prepared.reference,
        history_before,
    }
}

#[tokio::test]
async fn remote_target_becoming_history_after_claim_retains_replayable_transition_refusal() {
    use crate::workflow::LocalAuthorityResult;
    use std::sync::Arc;
    use tokio::sync::Notify;

    let TargetHistoryRaceFixture {
        db,
        authority,
        owner,
        reference,
        history_before,
    } = target_history_race_fixture().await;
    let claimed = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let pause = RemoteGoalClaimPause {
        claimed: Arc::clone(&claimed),
        resume: Arc::clone(&resume),
    };
    let apply_db = db.clone();
    let apply_authority = authority.clone();
    let apply = tokio::spawn(async move {
        apply_db
            .apply_remote_coordinator_goal_command_with_claim_commit(
                &apply_authority,
                reference,
                false,
                || Ok(at()),
                || panic!("the target remains classified by a retained refusal"),
                RemoteGoalClaimCommitCut::None,
                Some(pause),
            )
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(30), claimed.notified())
        .await
        .expect("remote Goal claim did not commit within 30 seconds");
    sqlx::query(
        "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=? AND kind='ordinary'",
    )
    .bind(owner.as_str())
    .execute(db.pool())
    .await
    .unwrap();
    resume.notify_one();

    let LocalAuthorityResult::DurableFactEstablished(Ok(RemoteGoalApplyOutcome::Receipt(receipt))) =
        apply.await.unwrap()
    else {
        panic!("target-unavailable claimed action must retain a refusal");
    };
    assert_eq!(receipt.request_id, reference);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?",
        )
        .bind(owner.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        history_before
    );
    assert!(matches!(
        &receipt.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Transition,
            observed_version: GoalVersion::INITIAL,
            ..
        }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts WHERE preparation_reference=?",
        )
        .bind(reference.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        db.apply_remote_coordinator_goal_command(
            &authority,
            reference,
            false,
            || panic!("receipt replay must not consult the clock"),
            || panic!("receipt replay retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        RemoteGoalApplyOutcome::Receipt(receipt)
    );
}

#[tokio::test]
async fn remote_post_claim_failure_is_unclassified_and_closes_receiptless_retry() {
    remote_unclassified_claim_closes_receiptless_retry(RemoteGoalClaimCommitCut::None).await;
}

#[tokio::test]
async fn remote_claim_lost_commit_ack_is_unclassified_and_closes_receiptless_retry() {
    remote_unclassified_claim_closes_receiptless_retry(
        RemoteGoalClaimCommitCut::LostAcknowledgement,
    )
    .await;
}

#[tokio::test]
async fn rolled_back_remote_claim_is_proven_prepared_and_can_be_applied_once() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-claim-rollback").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("claim rollback"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let before = counts(&db).await;
    let result = db
        .apply_remote_coordinator_goal_command_with_claim_commit(
            &authority,
            prepared.reference,
            false,
            || panic!("a rolled-back claim must not execute"),
            || panic!("a freshly proven prepared action retains authority"),
            RemoteGoalClaimCommitCut::RolledBack,
            None,
        )
        .await
        .established()
        .unwrap();
    assert!(matches!(
        result,
        Err(GoalDbError::Sqlx(sqlx::Error::WorkerCrashed))
    ));
    let state: String = sqlx::query_scalar(
        "SELECT apply_state FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state, "prepared");
    assert_eq!(counts(&db).await, before);
    assert!(db
        .retained_remote_coordinator_goal_command(&authority, prepared.reference)
        .await
        .unwrap()
        .is_none());
    let receipt = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || Ok(at()),
            || panic!("retry after a proven rollback retains authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(receipt) = receipt else {
        panic!("retry after rollback must return its receipt");
    };
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
    assert_eq!(
        db.conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unreadable_remote_claim_commit_classification_is_unclassified() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-claim-unreadable").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("unreadable claim"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(
        db.claim_remote_coordinator_goal_preparation(
            &authority,
            prepared.reference,
            RemoteGoalClaimCommitCut::None,
        )
        .await
        .unwrap(),
        RemoteCoordinatorGoalClaim::Claimed(_)
    ));
    db.pool().close().await;
    assert!(matches!(
        db.classify_remote_coordinator_goal_claim(
            &authority,
            prepared.reference,
            GoalDbError::Invalid("injected lost claim commit acknowledgement"),
        )
        .await
        .unwrap(),
        RemoteCoordinatorGoalClaim::Unclassified
    ));
}

#[tokio::test]
async fn remote_disabled_auto_continue_is_a_terminal_replayable_rejection() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-disabled-auto-continue").await;
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("disabled auto-continue"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    let before = counts(&db).await;
    let recorded_at = at();
    let receipt = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || Ok(recorded_at),
            || panic!("a deterministic create refusal must retain authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(receipt) = receipt else {
        panic!("classified refusal must return its receipt");
    };
    assert_eq!(
        receipt.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Transition,
            observed_version: GoalVersion::INITIAL,
            recorded_at,
        }
    );
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .unwrap();
    assert_eq!(
        db.apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || panic!("a rejected claim must never execute again"),
            || panic!("replaying a rejection must retain authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        RemoteGoalApplyOutcome::Receipt(receipt.clone())
    );
    assert_eq!(counts(&db).await, before);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    assert_eq!(
        db.retained_remote_coordinator_goal_command(&authority, prepared.reference)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn remote_apply_retains_receipt_and_history_decodes_remote_coordinator() {
    let db = fixture().await;
    let (authority, source, owner) = remote_source(&db, "remote-apply-replay").await;
    let enrollment_id: Uuid = sqlx::query_scalar::<_, String>(
        "SELECT id FROM federation_enrollments WHERE caller_instance_id=?",
    )
    .bind(source.instance_id.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap()
    .parse()
    .unwrap();
    let prepared = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &owner,
            create_intent("remote applied objective"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(db
        .retained_remote_coordinator_goal_command(&authority, prepared.reference)
        .await
        .unwrap()
        .is_none());
    let first = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let RemoteGoalApplyOutcome::Receipt(first) = first else {
        panic!("applied action must return its receipt");
    };
    assert!(matches!(&first.result, GoalUserResult::Accepted { .. }));
    match db
        .claim_remote_coordinator_goal_preparation(
            &authority,
            prepared.reference,
            RemoteGoalClaimCommitCut::None,
        )
        .await
        .unwrap()
    {
        RemoteCoordinatorGoalClaim::Replay(receipt) => assert_eq!(receipt, first),
        RemoteCoordinatorGoalClaim::Claimed(_)
        | RemoteCoordinatorGoalClaim::Expired { .. }
        | RemoteCoordinatorGoalClaim::Unclassified => {
            panic!("a receipt committed after an empty preflight must win claim arbitration")
        }
    }
    assert_eq!(
        db.retained_remote_coordinator_goal_command(&authority, prepared.reference)
            .await
            .unwrap(),
        Some(first.clone())
    );
    let replay = db
        .apply_remote_coordinator_goal_command(
            &authority,
            prepared.reference,
            false,
            || panic!("retained receipt must bypass clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(replay, RemoteGoalApplyOutcome::Receipt(first));
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    let expected_actor = phoenix_core::domain::conversation_goal::GoalActor::RemoteCoordinator(
        phoenix_core::domain::conversation_goal::EnrolledRemoteGoalAgentSource::new(
            source.instance_id,
            enrollment_id,
            source.source.clone(),
        ),
    );
    assert_eq!(
        history[0].mutation().attribution.requested_by,
        expected_actor
    );
    assert_eq!(history[0].mutation().attribution.applied_by, expected_actor);
    let replacement = FederationCredentialVerifier::from_bearer(b"receipt replacement credential");
    db.replace_federation_enrollment(
        source.instance_id,
        &"receipt replacement peer".parse().unwrap(),
        &replacement,
    )
    .await
    .unwrap();
    let replacement_authority =
        crate::FederationSendAuthority::new(source.instance_id, replacement);
    assert!(db
        .retained_remote_coordinator_goal_command(&authority, prepared.reference)
        .await
        .is_err());
    assert!(db
        .retained_remote_coordinator_goal_command(&replacement_authority, prepared.reference)
        .await
        .is_err());
    db.revoke_federation_enrollment(source.instance_id)
        .await
        .unwrap();
    assert!(db
        .retained_remote_coordinator_goal_command(&replacement_authority, prepared.reference)
        .await
        .is_err());
    let retained_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts WHERE preparation_reference=?",
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(retained_receipts, 1);
}

#[tokio::test]
async fn prepare_accepts_current_in_flight_prepare_goal_without_agent_message() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-in-flight").await;
    let (_, owner) = ordinary(&db, "prep-in-flight-owner").await;
    remove_source_message(&db, &caller, &source_call).await;
    let state = executing_source_state(
        source_tool(&source_call.tool_use_id, "prepare_goal"),
        vec![],
        &source_call.message_id,
        &source_call.tool_use_id,
        "prepare_goal",
    );
    db.update_conversation_state(&caller, &state).await.unwrap();

    let prepared = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("in-flight source"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();

    assert_eq!(
        prepared.source.source().message_id.as_str(),
        source_call.message_id
    );
    assert_eq!(
        prepared.source.source().tool_use_id.as_str(),
        source_call.tool_use_id
    );
    assert_eq!(preparation_count(&db).await, 1);
}

#[tokio::test]
async fn prepare_rejects_mismatched_or_non_current_in_flight_source() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-invalid-flight").await;
    let (_, owner) = ordinary(&db, "prep-invalid-flight-owner").await;
    remove_source_message(&db, &caller, &source_call).await;

    let wrong_name = executing_source_state(
        source_tool(&source_call.tool_use_id, "apply_goal"),
        vec![],
        &source_call.message_id,
        &source_call.tool_use_id,
        "prepare_goal",
    );
    assert_in_flight_source_refused(&db, &caller, &owner, &source_call, turn, &wrong_name).await;

    let queued_not_current = executing_source_state(
        source_tool("other-current-tool", "prepare_goal"),
        vec![source_tool(&source_call.tool_use_id, "prepare_goal")],
        &source_call.message_id,
        &source_call.tool_use_id,
        "prepare_goal",
    );
    assert_in_flight_source_refused(
        &db,
        &caller,
        &owner,
        &source_call,
        turn,
        &queued_not_current,
    )
    .await;

    let wrong_assistant = executing_source_state(
        source_tool(&source_call.tool_use_id, "prepare_goal"),
        vec![],
        "other-assistant-message",
        &source_call.tool_use_id,
        "prepare_goal",
    );
    assert_in_flight_source_refused(&db, &caller, &owner, &source_call, turn, &wrong_assistant)
        .await;

    for (block_id, block_name) in [
        ("other-tool-use", "prepare_goal"),
        (source_call.tool_use_id.as_str(), "apply_goal"),
    ] {
        let mismatched_block = executing_source_state(
            source_tool(&source_call.tool_use_id, "prepare_goal"),
            vec![],
            &source_call.message_id,
            block_id,
            block_name,
        );
        assert_in_flight_source_refused(
            &db,
            &caller,
            &owner,
            &source_call,
            turn,
            &mismatched_block,
        )
        .await;
    }

    let cancelling = crate::ConvState::CancellingTool {
        tool_use_id: source_call.tool_use_id.clone(),
        cause: CancelCause::UserRequested,
        skipped_tools: vec![],
        completed_results: vec![],
        assistant_message: source_assistant(&source_call, &source_call.message_id, "prepare_goal"),
        pending_sub_agents: vec![],
    };
    assert_in_flight_source_refused(&db, &caller, &owner, &source_call, turn, &cancelling).await;
}

#[tokio::test]
async fn goal_preparation_migration_follows_conversation_tool_migrations() {
    let db = fixture().await;
    let migrations: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,name FROM _migrations WHERE version>=143 ORDER BY version")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        migrations,
        vec![
            (143, "agent_creation_provenance".into()),
            (144, "product_conversation_rename_receipts".into()),
            (145, "coordinator_goal_preparations".into()),
            (146, "retain_product_conversation_rename_receipts".into()),
            (147, "retain_product_creation_receipts".into()),
            (148, "product_creation_initial_execution_receipts".into()),
            (149, "product_creation_image_retirement".into()),
            (150, "federated_subscription_terminal_cancellation".into()),
            (151, "one_shot_conversation_reminders".into()),
            (152, "goal_preparation_inflight_source".into()),
            (153, "close_writer_observations".into()),
            (155, "ordinary_self_goal_admission".into()),
            (156, "remote_coordinator_goals".into()),
            (158, "federated_conversation_actions".into()),
            (159, "federated_continuation_receipts".into()),
            (160, "slack_trigger_emojis".into()),
        ]
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn prepare_retains_exact_source_and_payload_without_mutating_or_dispatching() {
    let db = fixture().await;
    let (caller, source_product, source_call, turn) = coordinator_source(&db, "prep-retry").await;
    let (transcript, owner) = ordinary(&db, "prep-retry-owner").await;
    let before = counts(&db).await;
    let turns_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("first objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert_eq!(action.owner, owner);
    assert_eq!(action.expected_version, GoalVersion::INITIAL);
    assert_eq!(action.observed_goal_id, None);
    assert_eq!(
        action.source.source().product_conversation_id,
        source_product
    );
    assert_eq!(action.source.source().transcript_id.as_str(), caller);
    assert_eq!(
        action.source.source().execution.turn_id(),
        i64::try_from(turn.0).unwrap()
    );
    assert_eq!(
        action.source.source().message_id.as_str(),
        source_call.message_id
    );
    assert_eq!(
        action.source.source().tool_use_id.as_str(),
        source_call.tool_use_id
    );
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_turns")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        turns_before
    );
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());

    let intervention = crate::conversation_goals::user_commands::GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: crate::conversation_goals::user_commands::GoalUserChange::Create {
            objective: GoalText::parse("intervening objective").unwrap(),
        },
    };
    db.apply_user_goal_command(
        &intervention,
        &transcript,
        || Ok(at()),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let after = counts(&db).await;
    let retry = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("first objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert_eq!(retry.reference, action.reference);
    assert_eq!(retry.expected_version, action.expected_version);
    assert_eq!(retry.intent, action.intent);
    let fetched = db
        .read_coordinator_goal_preparation(&caller, action.reference)
        .await
        .unwrap();
    assert_eq!(fetched.reference, action.reference);
    assert_eq!(fetched.source, action.source);
    assert_eq!(counts(&db).await, after);
    assert_eq!(preparation_count(&db).await, 1);
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("changed objective")
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(preparation_count(&db).await, 1);
}

#[tokio::test]
async fn preparation_acknowledged_rollback_is_established_and_preserves_retained_action() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-rollback").await;
    let (_, owner) = ordinary(&db, "prep-rollback-owner").await;
    let retained = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("original"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let before = counts(&db).await;
    let conflict = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("changed"),
        )
        .await;
    assert!(matches!(
        conflict,
        crate::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
            GoalDbError::RetryConflict
        ))
    ));
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, retained.reference)
            .await
            .unwrap(),
        retained
    );
}

#[tokio::test]
async fn preparation_lost_commit_ack_is_unclassified_and_source_retry_recovers_one_action() {
    let db = fixture().await;
    let before = counts(&db).await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-lost-ack").await;
    let (_, owner) = ordinary(&db, "prep-lost-ack-owner").await;
    let intent = create_intent("retained after lost commit acknowledgment");

    let result = db
        .prepare_coordinator_goal_action_with_commit(
            &caller,
            &owner,
            &source_call,
            turn,
            intent.clone(),
            PreparationCommit::LoseAcknowledgement,
        )
        .await;
    assert!(matches!(
        result,
        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    let retained_row =
        sqlx::query("SELECT p.*,NULL AS source_instance_id FROM conversation_goal_preparations p")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let retained = decode(&retained_row).unwrap();

    let recovered = db
        .prepare_coordinator_goal_action(&caller, &owner, &source_call, turn, intent)
        .await
        .established()
        .expect("source retry commit was acknowledged")
        .unwrap();
    assert_eq!(recovered, retained);
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, recovered.reference)
            .await
            .unwrap(),
        recovered
    );
}

#[tokio::test]
async fn prepared_command_observes_version_cas_and_replays_retained_refusal() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-cas").await;
    let (transcript, owner) = ordinary(&db, "prep-cas-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("stale objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    let intervention = crate::conversation_goals::user_commands::GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: crate::conversation_goals::user_commands::GoalUserChange::Create {
            objective: GoalText::parse("newer objective").unwrap(),
        },
    };
    db.apply_user_goal_command(
        &intervention,
        &transcript,
        || Ok(at()),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(
        receipt.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Version,
            ..
        }
    ));
    let before = counts(&db).await;
    let retry = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || panic!("retained refusal must not re-clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.conversation_goal(&owner)
            .await
            .unwrap()
            .unwrap()
            .objective()
            .as_str(),
        "newer objective"
    );
}

#[tokio::test]
async fn prepared_command_accepts_real_coordinator_source_and_replays_without_mutation() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-accepted").await;
    let (transcript, owner) = ordinary(&db, "prep-accepted-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("coordinator objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    assert_eq!(command.request_id, action.reference);
    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(matches!(&history[0].mutation().attribution.requested_by,
        phoenix_core::domain::conversation_goal::GoalActor::Coordinator(source) if source == action.source.source()));
    let before = counts(&db).await;
    let retry = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || panic!("accepted replay must not re-clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, before);
}

#[tokio::test]
async fn preparation_requires_exact_server_turn_and_current_coordinator() {
    let db = fixture().await;
    let (ordinary_id, owner) = ordinary(&db, "prepared-authority-target").await;
    let (caller, _, call, turn) = coordinator_source(&db, "prepared-authority").await;
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            DirectTurnTurnId(0),
            create_intent("wrong execution"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::TargetChanged)
    ));
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &ordinary_id,
            &owner,
            &call,
            turn,
            create_intent("wrong role"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(preparation_count(&db).await, 0);
    let prepared = db
        .prepare_coordinator_goal_action(&caller, &owner, &call, turn, create_intent("retained"))
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            DirectTurnTurnId(0),
            create_intent("retained"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, prepared.reference)
            .await
            .unwrap()
            .expected_version,
        GoalVersion::INITIAL
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn coordinator_payload_and_source_are_checked_before_busy_and_receipt_replay() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-poison").await;
    let (transcript, owner) = ordinary(&db, "prep-poison-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("authorized objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();

    let mut poisoned = action.command(command.principal_id).unwrap();
    poisoned.change = crate::conversation_goals::user_commands::GoalUserChange::Create {
        objective: GoalText::parse("poison objective").unwrap(),
    };
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &poisoned,
            &transcript,
            true,
            || Ok(at()),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    let mut wrong_source = action.source.source().clone();
    wrong_source.tool_use_id = GoalText::parse("other-tool-use").unwrap();
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            &wrong_source,
            &command,
            &transcript,
            true,
            || Ok(at()),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );

    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            action.source.source(),
            &poisoned,
            &transcript,
            false,
            || panic!("poisoned replay must not clock"),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        db.retained_coordinator_goal_command(&caller, &command)
            .await
            .unwrap(),
        Some(receipt)
    );
}

#[tokio::test]
async fn preparation_schema_rejects_update_delete_replace_and_malformed_shape() {
    let db = fixture().await;
    let (_, owner) = ordinary(&db, "prepared-schema-target").await;
    let (caller, _, call, turn) = coordinator_source(&db, "prepared-schema").await;
    let prepared = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            turn,
            create_intent("immutable action"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert!(sqlx::query(
        "UPDATE conversation_goal_preparations SET objective='overwrite' WHERE reference=?"
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM conversation_goal_preparations WHERE reference=?")
            .bind(prepared.reference.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_preparations SELECT * FROM conversation_goal_preparations WHERE reference=?")
        .bind(prepared.reference.as_bytes().as_slice()).execute(db.pool()).await.is_err());
    assert!(sqlx::query("INSERT INTO conversation_goal_preparations (reference,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,operation,expected_version,observed_goal_id,objective,disposition,reason) SELECT ?,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'malformed-shape','create',1,NULL,objective,NULL,NULL FROM conversation_goal_preparations WHERE reference=?")
        .bind(Uuid::new_v4().as_bytes().as_slice()).bind(prepared.reference.as_bytes().as_slice()).execute(db.pool()).await.is_err());
    assert_eq!(preparation_count(&db).await, 1);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
}

#[tokio::test]
async fn preparation_retains_source_locators_after_message_deletion_and_owner_cascade() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "retained-source").await;
    let (transcript, owner) = ordinary(&db, "retained-source-owner").await;
    let prepared = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("retained source"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = prepared.command(Uuid::new_v4()).unwrap();
    db.apply_coordinator_goal_command(
        &caller,
        prepared.source.source(),
        &command,
        &transcript,
        false,
        || Ok(at()),
        || panic!("unexpected source failure"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    sqlx::query("DELETE FROM messages WHERE message_id=?")
        .bind(&source_call.message_id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, prepared.reference)
            .await
            .unwrap(),
        prepared
    );
    let replay = db
        .retained_coordinator_goal_command(&caller, &command)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(replay.result, GoalUserResult::Accepted { .. }));
    db.delete_conversation(&transcript).await.unwrap();
    assert_eq!(preparation_count(&db).await, 0);
}
