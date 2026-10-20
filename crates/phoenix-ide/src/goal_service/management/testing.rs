use super::GoalManagementService;
use crate::runtime::{traits::ActiveDirectTurn, RuntimeManager};
use phoenix_core::domain::{
    db_schema::InputOrigin,
    sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    },
    sm_state::ConvState,
};
use phoenix_db::{
    workflow::{
        AcceptAuthoritativeTurn, ClaimAuthoritativeTurnInput, LocalAuthorityResult,
        MaterializeAuthoritativeTurnInput, MaterializeAuthoritativeTurnOutcome,
    },
    Database,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};
use std::sync::Arc;

pub(crate) struct Fixture {
    pub(crate) home: tempfile::TempDir,
    pub(crate) source: String,
    pub(crate) runtime: Arc<RuntimeManager>,
    pub(crate) service: GoalManagementService,
    pub(crate) target: phoenix_core::domain::product_conversation::ProductConversationId,
    pub(crate) active: ActiveDirectTurn,
}

pub(crate) async fn fixture(coordinator: bool) -> Fixture {
    let db = Database::open_in_memory().await.unwrap();
    let target = db
        .create_conversation("management-target", "target", "/tmp", true, None, None)
        .await
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let source = if coordinator {
        db.initialize_coordinator_work_scope(home.path().to_str().unwrap())
            .await
            .unwrap();
        db.get_or_create_coordinator(
            Some("test-model"),
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap()
        .id
    } else {
        db.create_conversation("management-source", "ordinary", "/tmp", true, None, None)
            .await
            .unwrap()
            .id
    };
    let db = db.install_goal_controls_for_tests().await.unwrap();
    if !coordinator {
        release_ordinary_source_control(&db, &source).await;
    }
    let prepared = prepared_input();
    let repo = db.workflow_repository();
    let accepted = repo
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("management-turn").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(source.clone()),
                prepared.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("accepted")
    };
    let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
    let claim = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: phoenix_workflow::ProcessIncarnation(1),
            now: Timestamp(2),
            lease_until: phoenix_workflow::LeaseExpiry(100),
        })
        .await
        .unwrap();
    let LocalAuthorityResult::DurableFactEstablished(
        MaterializeAuthoritativeTurnOutcome::Materialized(materialized),
    ) = repo
        .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
            turn_id,
            authority: claim.authority.unwrap(),
            prepared,
            sequence_id: 1,
            created_at: Timestamp(3),
            accepted_state: ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: chrono::Utc::now(),
            now: Timestamp(3),
        })
        .await
    else {
        panic!("materialized")
    };
    let active = crate::runtime::traits::ActiveDirectTurn {
        turn_id,
        generation: materialized.generation,
    };
    let runtime = crate::goal_service::tests::test_runtime_manager(db);
    Fixture {
        home,
        source,
        service: GoalManagementService::new(Arc::downgrade(&runtime)),
        runtime,
        target: target.product_conversation_id,
        active,
    }
}

async fn release_ordinary_source_control(db: &Database, source: &str) {
    use phoenix_core::domain::conversation_goal::GoalTimestampUs;
    use phoenix_db::conversation_goals::control::{ControlCommand, ControlOutcome, ControlRequest};
    let owner = db
        .get_conversation(source)
        .await
        .unwrap()
        .product_conversation_id;
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT control_version FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_optional(db.pool())
    .await
    .unwrap();
    let expected_control_version = u64::try_from(version.unwrap_or(0)).unwrap();
    let receipt = db
        .apply_goal_control(
            &ControlRequest {
                owner,
                principal_id: uuid::Uuid::parse_str(&db.instance_id().await.unwrap().to_string())
                    .unwrap(),
                request_id: uuid::Uuid::new_v4(),
                command: ControlCommand::Release,
                expected_control_version,
            },
            GoalTimestampUs::parse(1).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(receipt.result, ControlOutcome::Accepted { snapshot } if !snapshot.stopped()));
}

fn prepared_input() -> PreparedDirectTurnPayload {
    PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            origin: InputOrigin::UnknownHistorical,
            text: "manage goal".into(),
            images: vec![],
            files: vec![],
            message_id: "management-input".into(),
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "manage goal".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    )
}
