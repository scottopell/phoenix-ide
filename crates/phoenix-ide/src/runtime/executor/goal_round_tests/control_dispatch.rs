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

#[tokio::test]
async fn conversation_goal_stop_after_model_admission_before_first_poll_prevents_external_dispatch()
{
    for with_goal in [false, true] {
        let (db, owner, active) = fixture(with_goal).await;
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::new());
        let mut rt = runtime(
            db.clone(),
            dir.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(true)),
        )
        .with_active_direct_turn(Some(active));
        let barrier = Arc::new(GoalDispatchTestBarrier::default());
        rt.goal_dispatch_barrier = Some(barrier.clone());
        rt.state = ConvState::LlmRequesting { attempt: 1 };
        rt.execute_effect(Effect::RequestLlm).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), barrier.reached.notified())
            .await
            .unwrap();
        assert!(client.requests.lock().unwrap().is_empty());
        control(&db, &owner, ControlCommand::Stop, 1).await;
        control(&db, &owner, ControlCommand::Release, 2).await;
        barrier.resume.notify_one();
        tokio::time::timeout(Duration::from_secs(5), rt.llm_task_handle.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let (generation, outcome) =
            tokio::time::timeout(Duration::from_secs(5), rt.llm_outcome_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(outcome, LlmOutcome::Cancelled));
        rt.process_generation_tagged_llm_outcome(generation, outcome)
            .await;
        assert!(matches!(rt.state, ConvState::Idle));
        assert!(
            client.requests.lock().unwrap().is_empty(),
            "old execution cannot be revived by release"
        );
        let metrics: i64 = sqlx::query_scalar("SELECT count(*) FROM llm_request_metrics")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(
            metrics, 0,
            "refusal before first provider poll is not an attempt"
        );
        assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
    }
}

#[tokio::test]
async fn conversation_goal_stop_after_tool_policy_before_external_poll_prevents_tool_dispatch() {
    use super::super::MCP_TOOL;
    for with_goal in [false, true] {
        let (db, owner, active) = fixture(with_goal).await;
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::scripted(vec![journey_response(
            vec![ContentBlock::ToolUse {
                id: "barrier-tool".into(),
                name: MCP_TOOL.into(),
                input: serde_json::json!({}),
            }],
            false,
        )]));
        let tools = Arc::new(DeferredMcpCatalog::new(true));
        let mut rt = runtime(db.clone(), dir.path(), client, tools.clone())
            .with_active_direct_turn(Some(active));
        rt.state = ConvState::LlmRequesting { attempt: 1 };
        dispatch(&mut rt).await;
        let barrier = Arc::new(GoalDispatchTestBarrier::default());
        rt.goal_dispatch_barrier = Some(barrier.clone());
        let task = tokio::spawn(async move {
            journey_provider_step(&mut rt).await;
            rt
        });
        tokio::time::timeout(Duration::from_secs(5), barrier.reached.notified())
            .await
            .unwrap();
        assert!(tools.recorded_executions().is_empty());
        control(&db, &owner, ControlCommand::Stop, 1).await;
        control(&db, &owner, ControlCommand::Release, 2).await;
        barrier.resume.notify_one();
        let mut rt = task.await.unwrap();
        let (_, outcome) = tokio::time::timeout(Duration::from_secs(5), rt.tool_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            outcome,
            ToolExecOutcome::Aborted {
                reason: crate::state_machine::AbortReason::GoalControlRevoked,
                ..
            }
        ));
        assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
        rt.process_outcome(EffectOutcome::Tool(outcome))
            .await
            .unwrap();
        assert!(matches!(rt.state, ConvState::Idle));
        assert!(matches!(
            db.get_conversation(CONVERSATION).await.unwrap().state,
            ConvState::Idle
        ));
        assert!(tools.recorded_executions().is_empty());
    }
}

#[tokio::test]
async fn conversation_goal_stop_cancels_the_actual_inflight_provider_future() {
    use super::super::PendingExternalCall;
    for with_goal in [false, true] {
        let (db, owner, active) = fixture(with_goal).await;
        let turn_id = i64::try_from(active.turn_id.0).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::new());
        let pending = Arc::new(PendingExternalCall::default());
        *client.pending.lock().unwrap() = Some(pending.clone());
        let mut rt = runtime(
            db.clone(),
            dir.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(true)),
        )
        .with_active_direct_turn(Some(active));
        rt.state = ConvState::LlmRequesting { attempt: 1 };
        rt.execute_effect(Effect::RequestLlm).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), pending.entered.notified())
            .await
            .unwrap();
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        control(&db, &owner, ControlCommand::Stop, 1).await;
        tokio::time::timeout(Duration::from_secs(5), rt.llm_task_handle.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let (generation, outcome) =
            tokio::time::timeout(Duration::from_secs(5), rt.llm_outcome_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(outcome, LlmOutcome::Cancelled));
        rt.process_generation_tagged_llm_outcome(generation, outcome)
            .await;
        assert!(matches!(rt.state, ConvState::Idle));
        tokio::time::timeout(Duration::from_secs(5), pending.dropped.notified())
            .await
            .unwrap();
        let terminal: Option<String> =
            sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id=?")
                .bind(turn_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(terminal.as_deref(), Some("Cancelled"));
        let request_id = client.requests.lock().unwrap()[0]
            .telemetry
            .as_ref()
            .unwrap()
            .request_id
            .clone();
        let metrics = db
            .llm_request_metrics_for_request(&request_id)
            .await
            .unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(
            metrics[0].outcome,
            phoenix_core::domain::llm_types::LlmAttemptOutcome::Cancelled
        );
        control(&db, &owner, ControlCommand::Release, 2).await;
        assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_stop_cancels_the_actual_inflight_tool_future_and_context() {
    use super::super::{PendingExternalCall, MCP_TOOL};
    for (with_goal, fail_settlement) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let (db, owner, active) = fixture(with_goal).await;
        let turn_id = i64::try_from(active.turn_id.0).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(RenderingAnthropicClient::scripted(vec![journey_response(
            vec![ContentBlock::ToolUse {
                id: "inflight-tool".into(),
                name: MCP_TOOL.into(),
                input: serde_json::json!({}),
            }],
            false,
        )]));
        let pending = Arc::new(PendingExternalCall::default());
        let mut catalog = DeferredMcpCatalog::new(true);
        catalog.2 = Some(pending.clone());
        let tools = Arc::new(catalog);
        let mut rt = runtime(db.clone(), dir.path(), client, tools.clone())
            .with_active_direct_turn(Some(active));
        rt.state = ConvState::LlmRequesting { attempt: 1 };
        dispatch(&mut rt).await;
        journey_provider_step(&mut rt).await;
        assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
        tokio::time::timeout(Duration::from_secs(5), pending.entered.notified())
            .await
            .unwrap();
        assert_eq!(tools.recorded_executions().len(), 1);
        control(&db, &owner, ControlCommand::Stop, 1).await;
        let (_, outcome) = tokio::time::timeout(Duration::from_secs(5), rt.tool_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            outcome,
            ToolExecOutcome::Aborted {
                reason: crate::state_machine::AbortReason::GoalControlRevoked,
                ..
            }
        ));
        if fail_settlement {
            let before = db.get_conversation(CONVERSATION).await.unwrap().state;
            let before_messages =
                serde_json::to_value(db.get_messages(CONVERSATION).await.unwrap()).unwrap();
            sqlx::query("CREATE TRIGGER fail_revoked_tool_settlement BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'revoked tool settlement failed'); END")
                .execute(db.pool()).await.unwrap();
            assert!(rt
                .process_outcome(EffectOutcome::Tool(outcome))
                .await
                .is_err());
            assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
            assert_eq!(
                db.get_conversation(CONVERSATION).await.unwrap().state,
                before
            );
            assert_eq!(
                serde_json::to_value(db.get_messages(CONVERSATION).await.unwrap()).unwrap(),
                before_messages
            );
            assert!(
                rt.terminal_transition_retry.is_some(),
                "the sole revoked outcome must survive a failed write"
            );
            sqlx::query("DROP TRIGGER fail_revoked_tool_settlement")
                .execute(db.pool())
                .await
                .unwrap();
            rt.retry_terminal_transition().await;
            assert!(rt.terminal_transition_retry.is_none());
        } else {
            rt.process_outcome(EffectOutcome::Tool(outcome))
                .await
                .unwrap();
        }
        assert!(matches!(rt.state, ConvState::Idle));
        assert!(matches!(
            db.get_conversation(CONVERSATION).await.unwrap().state,
            ConvState::Idle
        ));
        let terminal: Option<String> =
            sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id=?")
                .bind(turn_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(terminal.as_deref(), Some("Cancelled"));
        tokio::time::timeout(Duration::from_secs(5), pending.dropped.notified())
            .await
            .unwrap();
        assert!(pending
            .cancel
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled());
        control(&db, &owner, ControlCommand::Release, 2).await;
        assert_eq!(tools.recorded_executions().len(), 1);
    }
}

#[tokio::test]
async fn conversation_goal_stop_waiting_for_database_does_not_hold_the_external_poll_gate() {
    use std::{future::Future, task::Poll};
    for with_goal in [false, true] {
        let (db, owner, active) = fixture(with_goal).await;
        let permit = db
            .admit_goal_control_dispatch(
                CONVERSATION,
                Some((i64::try_from(active.turn_id.0).unwrap(), active.generation)),
            )
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let operation = {
            let db = db.clone();
            let entered = entered.clone();
            let resume = resume.clone();
            tokio::spawn(async move {
                permit
                    .run(async move {
                        let tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
                        entered.notify_one();
                        resume.notified().await;
                        tx.commit().await.unwrap();
                    })
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let request = ControlRequest {
            owner: owner.clone(),
            principal_id: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            command: ControlCommand::Stop,
            expected_control_version: 1,
        };
        let mut stop =
            Box::pin(db.apply_goal_control(&request, GoalTimestampUs::parse(11).unwrap()));
        std::future::poll_fn(|cx| {
            assert!(
                stop.as_mut().poll(cx).is_pending(),
                "active operation still owns the sole DB connection"
            );
            Poll::Ready(())
        })
        .await;
        resume.notify_one();
        let (stopped, operation) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(stop, operation)
        })
        .await
        .unwrap();
        stopped.unwrap();
        let _dispatch_result = operation.unwrap();
        assert!(db
            .admit_goal_control_dispatch(
                CONVERSATION,
                Some((i64::try_from(active.turn_id.0).unwrap(), active.generation))
            )
            .await
            .is_err());
    }
}
