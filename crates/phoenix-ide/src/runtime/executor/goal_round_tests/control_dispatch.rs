use super::*;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::control::{ControlCommand, ControlRequest};
use phoenix_db::workflow::{
    AcceptAuthoritativeTurn, ClaimAuthoritativeTurnInput, LocalAuthorityResult,
    MaterializeAuthoritativeTurnInput, MaterializeAuthoritativeTurnOutcome,
};

async fn control(
    db: &Database,
    owner: &ProductConversationId,
    command: ControlCommand,
    version: u64,
) {
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            command,
            expected_control_version: version,
        },
        GoalTimestampUs::parse(10).unwrap(),
    )
    .await
    .unwrap();
}

#[allow(clippy::too_many_lines)]
async fn fixture(
    with_goal: bool,
) -> (
    Database,
    ProductConversationId,
    crate::runtime::traits::ActiveDirectTurn,
) {
    let db = Database::open_in_memory().await.unwrap();
    let root = db
        .create_conversation(CONVERSATION, "installed dispatch", "/tmp", true, None, None)
        .await
        .unwrap();
    let owner = root.product_conversation_id;
    if with_goal {
        let actor = GoalActor::GeneratedHost {
            action_id: text("dispatch-fixture"),
        };
        db.apply_conversation_goal(
            &GoalMutation {
                owner: owner.clone(),
                expected_version: GoalVersion::INITIAL,
                action_id: text("dispatch-goal-create"),
                attribution: GoalAttribution {
                    requested_by: actor.clone(),
                    applied_by: actor,
                },
                change: GoalChange::Create {
                    goal_id: GoalId::parse("dispatch-goal").unwrap(),
                    objective: text("exercise installed dispatch"),
                },
            },
            CONVERSATION,
            GoalTimestampUs::parse(10).unwrap(),
        )
        .await
        .unwrap();
    }
    let db = db.install_goal_controls_for_tests().await.unwrap();
    control(&db, &owner, ControlCommand::Release, 0).await;
    let submitted = SubmittedDirectTurnIdentity {
        origin: InputOrigin::UnknownHistorical,
        text: "test admitted dispatch".into(),
        images: vec![],
        files: vec![],
        message_id: "dispatch-input".into(),
        user_agent: None,
        skill_invocation: None,
        expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
    };
    let delivery = PreparedDirectTurnDelivery {
        text: submitted.text.clone(),
        llm_text: None,
        images: vec![],
        files: vec![],
        user_agent: None,
        skill_invocation: None,
    };
    let prepared = PreparedDirectTurnPayload::from_parts(submitted, delivery);
    let repo = db.workflow_repository();
    let accepted = repo
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("installed-dispatch").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(CONVERSATION.into()),
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
    (db, owner, active)
}

#[tokio::test]
async fn conversation_goal_installed_runtime_dispatch_checks_control_without_relying_on_an_emitter()
{
    for with_goal in [false, true] {
        let (db, owner, active) = fixture(with_goal).await;
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::new());
        let tools = Arc::new(DeferredMcpCatalog::new(false));
        let mut rt = runtime(db.clone(), dir.path(), client.clone(), tools.clone())
            .with_active_direct_turn(Some(active));
        assert!(
            rt.goal_report_emitter.is_none(),
            "installation does not activate the private report declaration"
        );
        dispatch(&mut rt).await;
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        control(&db, &owner, ControlCommand::Stop, 1).await;
        assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
        assert!(rt.llm_task_handle.is_none());
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let tool = ToolCall::new(
            "after-stop",
            ToolInput::Think(ThinkInput {
                thoughts: "must not run".into(),
            }),
        );
        assert!(rt
            .execute_effect(Effect::ExecuteTool { tool })
            .await
            .is_err());
        assert!(tools.recorded_executions().is_empty());
        control(&db, &owner, ControlCommand::Release, 2).await;
        assert!(
            rt.execute_effect(Effect::RequestLlm).await.is_err(),
            "release cannot recapture an old execution"
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn conversation_goal_installed_runtime_invalid_authority_does_not_reach_the_provider() {
    enum Refusal {
        MissingTurn,
        MissingSchema,
        StaleGeneration,
    }
    for refusal in [
        Refusal::MissingTurn,
        Refusal::MissingSchema,
        Refusal::StaleGeneration,
    ] {
        let (db, _, mut active) = fixture(false).await;
        let active = match refusal {
            Refusal::MissingTurn => None,
            Refusal::MissingSchema => {
                sqlx::query("DROP TABLE conversation_goal_control_bindings")
                    .execute(db.pool())
                    .await
                    .unwrap();
                Some(active)
            }
            Refusal::StaleGeneration => {
                active.generation += 1;
                Some(active)
            }
        };
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::new());
        let mut rt = runtime(
            db.clone(),
            dir.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(false)),
        )
        .with_active_direct_turn(active);
        assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
        assert!(rt.llm_task_handle.is_none());
        assert!(client.requests.lock().unwrap().is_empty());
        assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 1);
    }
}
