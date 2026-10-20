use super::*;
use phoenix_core::domain::{
    conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalDisposition, GoalId, GoalMutation, GoalText,
        GoalTimestampUs, GoalVersion,
    },
    sm_event::PreparedDirectTurnPayload,
    sm_state::ConvState,
};
use phoenix_db::workflow::{ClaimAuthoritativeTurnInput, MaterializeAuthoritativeTurnInput};
use phoenix_workflow::{ConversationAuthority, LeaseExpiry, ProcessIncarnation, Timestamp};

pub(crate) const OBJECTIVE: &str = "PRIVATE_CAPTURE_OBJECTIVE_461d";
pub(crate) const REASON: &str = "PRIVATE_CAPTURE_BLOCKED_REASON_7c90";
pub(crate) const GOAL_ID: &str = "PRIVATE_CAPTURE_GOAL_ID_9b26";

pub(crate) fn assert_private_absent(value: &serde_json::Value) {
    let text = value.to_string();
    for secret in [OBJECTIVE, REASON, GOAL_ID] {
        assert!(
            !text.contains(secret),
            "saved fact leaked into shared wire: {secret}"
        );
    }
}

async fn captured_wait_event(db: &phoenix_db::Database) -> (PendingWatchEvent, String) {
    let installed = db.clone().install_goal_controls_for_tests().await.unwrap();
    let source = installed
        .create_conversation(
            "capture-privacy",
            "Capture privacy",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    for (version, change) in [
        (
            0,
            GoalChange::Create {
                goal_id: GoalId::parse(GOAL_ID).unwrap(),
                objective: GoalText::parse(OBJECTIVE).unwrap(),
            },
        ),
        (
            1,
            GoalChange::SetDisposition {
                goal_id: GoalId::parse(GOAL_ID).unwrap(),
                disposition: GoalDisposition::Blocked(GoalText::parse(REASON).unwrap()),
            },
        ),
    ] {
        let key = format!("capture-privacy-{version}");
        let actor = GoalActor::GeneratedHost {
            action_id: GoalText::parse(&key).unwrap(),
        };
        installed
            .apply_conversation_goal(
                &GoalMutation {
                    owner: source.product_conversation_id.clone(),
                    expected_version: GoalVersion::parse(version).unwrap(),
                    action_id: GoalText::parse(&key).unwrap(),
                    attribution: GoalAttribution {
                        requested_by: actor.clone(),
                        applied_by: actor,
                    },
                    change,
                },
                &source.id,
                GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros()).unwrap(),
            )
            .await
            .unwrap();
    }
    installed
        .watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    installed
        .update_conversation_state(
            &source.id,
            &ConvState::AwaitingTaskApproval {
                task_file: "tasks/12345-p1-ready--capture.md".into(),
                title: "Review".into(),
                priority: phoenix_core::task_source::Priority::P1,
                plan: "Review".into(),
            },
        )
        .await
        .unwrap();
    let events = installed
        .pending_coordinator_watch_events(16)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert!(matches!(
        event.goal_capture,
        Some(phoenix_core::domain::watch_goal::WatchGoalCapture::CapturedSavedGoal { .. })
    ));
    (event.clone(), source.id)
}

pub(crate) async fn captured_notification(
    db: &phoenix_db::Database,
) -> (phoenix_db::Message, String, String) {
    let (event, source_id) = captured_wait_event(db).await;
    let diagnostic = notification(&event);
    let mut legacy = event.clone();
    legacy.goal_capture = None;
    assert_eq!(diagnostic, notification(&legacy));
    assert_private_absent(&serde_json::json!(diagnostic));
    let target = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        Arc::new(phoenix_llm::ModelRegistry::new_empty()),
        crate::platform::PlatformCapability::None {
            details: "test".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    let service = SendChatApplicationService::new(db.clone(), runtime);
    let request = || SendChatRequest {
        conversation_id: target.id.clone(),
        origin: InputOrigin::SubscriptionEvent {
            event_id: event.event_id.clone(),
        },
        text: diagnostic.clone(),
        message_id: event.event_id.clone(),
        images: vec![],
        files: vec![],
        user_agent: None,
        expansion_policy: MessageExpansionPolicy::LiteralText,
    };
    assert!(matches!(
        service.send(request()).await.unwrap(),
        SendChatOutcome::Delivered
    ));
    let repo = db.workflow_repository();
    let turns = repo
        .list_discoverable_accepted_turns(&ConversationAuthority(target.id.clone()))
        .await
        .unwrap();
    assert_eq!(turns.len(), 1);
    let (turn_id, workflow_id) = turns[0];
    let turn = repo
        .load_authoritative_turn(turn_id)
        .await
        .unwrap()
        .unwrap();
    let prepared = PreparedDirectTurnPayload::from_exact_bytes(turn.prepared.payload()).unwrap();
    assert_private_absent(&serde_json::to_value(&prepared).unwrap());
    let now = chrono::Utc::now();
    let now_seconds = u64::try_from(now.timestamp()).unwrap();
    let claim = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(17),
            now: Timestamp(now_seconds),
            lease_until: LeaseExpiry(now_seconds + 60),
        })
        .await
        .unwrap();
    repo.materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
        turn_id,
        authority: claim.authority.unwrap(),
        prepared,
        sequence_id: 1,
        created_at: Timestamp(now_seconds),
        accepted_state: ConvState::LlmRequesting { attempt: 1 },
        state_updated_at: now,
        now: Timestamp(now_seconds),
    })
    .await
    .established()
    .unwrap();
    assert!(matches!(
        service.send(request()).await.unwrap(),
        SendChatOutcome::AlreadyPersisted
    ));
    let messages = db.get_messages(&target.id).await.unwrap();
    assert_eq!(messages.len(), 1);
    let message = messages.into_iter().next().unwrap();
    assert_private_absent(&serde_json::to_value(&message).unwrap());
    let phoenix_db::MessageContent::User(content) = &message.content else {
        panic!("notification user message");
    };
    assert_eq!(content.text, diagnostic);
    assert_eq!(content.llm_text(), diagnostic);
    (message, source_id, diagnostic)
}
