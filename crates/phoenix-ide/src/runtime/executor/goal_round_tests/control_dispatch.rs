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
pub(super) async fn fixture(
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
        let LlmCompletion {
            generation,
            outcome,
            ..
        } = tokio::time::timeout(Duration::from_secs(5), rt.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, LlmOutcome::GoalControlRevoked));
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
        let LlmCompletion {
            generation,
            outcome,
            ..
        } = tokio::time::timeout(Duration::from_secs(5), rt.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, LlmOutcome::GoalControlRevoked));
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
            metrics[0].metrics.outcome,
            phoenix_core::domain::llm_types::LlmAttemptOutcome::Cancelled
        );
        control(&db, &owner, ControlCommand::Release, 2).await;
        assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn conversation_goal_stop_cancels_the_actual_inflight_tool_future_and_context() {
    for (with_goal, fail_settlement) in [(false, false), (true, false), (false, true), (true, true)]
    {
        revoked_tool_scenario(with_goal, fail_settlement, false).await;
    }
}

#[tokio::test]
async fn conversation_goal_pending_report_and_inflight_mcp_revoke_in_one_atomic_retry() {
    revoked_tool_scenario(true, true, true).await;
}

#[allow(clippy::too_many_lines)]
async fn revoked_tool_scenario(with_goal: bool, fail_settlement: bool, pending_report: bool) {
    use super::super::{PendingExternalCall, MCP_TOOL};
    {
        let (db, owner, active) = fixture(with_goal).await;
        let turn_id = i64::try_from(active.turn_id.0).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut blocks = Vec::new();
        if pending_report {
            blocks.push(ContentBlock::ToolUse {
                id: "pending-report".into(), name: "goal_report".into(),
                input: serde_json::json!({"status":"complete","reason":"prepared before revocation"}),
            });
        }
        blocks.push(ContentBlock::ToolUse {
            id: "inflight-tool".into(),
            name: MCP_TOOL.into(),
            input: serde_json::json!({}),
        });
        let client = Arc::new(RenderingAnthropicClient::scripted(vec![journey_response(
            blocks, false,
        )]));
        let pending = Arc::new(PendingExternalCall::default());
        let mut catalog = DeferredMcpCatalog::new(true);
        catalog.2 = Some(pending.clone());
        let tools = Arc::new(catalog);
        let mut rt = runtime(db.clone(), dir.path(), client.clone(), tools.clone())
            .with_active_direct_turn(Some(active));
        if pending_report {
            let workflow_id: i64 =
                sqlx::query_scalar("SELECT workflow_id FROM durable_turns WHERE turn_id=?")
                    .bind(turn_id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            let execution = GoalExecution::parse(workflow_id, turn_id).unwrap();
            let binding = db
                .conversation_goal_execution_binding(execution)
                .await
                .unwrap()
                .unwrap();
            rt.goal_report_emitter = Some(GoalReportEmitter {
                binding: binding.try_into().unwrap(),
            });
        }
        rt.state = ConvState::LlmRequesting { attempt: 1 };
        dispatch(&mut rt).await;
        journey_provider_step(&mut rt).await;
        if pending_report {
            journey_report_step(&mut rt).await;
            assert!(
                matches!(&rt.state, ConvState::ToolExecuting { completed_results, .. }
                if matches!(completed_results.as_slice(), [ToolRoundResult::PendingGoalReport { .. }]))
            );
        }
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
            let before_replay = db.load_provider_replay_state(CONVERSATION).await.unwrap();
            let before_goal = db.conversation_goal(&owner).await.unwrap();
            let before_history = db
                .conversation_goal_history(&owner, GoalVersion::INITIAL)
                .await
                .unwrap();
            let mut publications = rt.broadcast_tx.subscribe();
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
            assert_eq!(
                db.load_provider_replay_state(CONVERSATION).await.unwrap(),
                before_replay
            );
            assert_eq!(db.conversation_goal(&owner).await.unwrap(), before_goal);
            assert_eq!(
                db.conversation_goal_history(&owner, GoalVersion::INITIAL)
                    .await
                    .unwrap(),
                before_history
            );
            while let Ok(event) = publications.try_recv() {
                assert!(!matches!(
                    event,
                    SseEvent::Message { .. } | SseEvent::StateChange { .. }
                ));
            }
            let ownership: (Option<String>, i64) = sqlx::query_as(
                "SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?",
            )
            .bind(turn_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(ownership, (None, 1));
            let obligations: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM direct_turn_terminal_obligations WHERE turn_id=?",
            )
            .bind(turn_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(obligations, 0);
            assert!(
                rt.terminal_transition_retry.is_some(),
                "the sole revoked outcome must survive a failed write"
            );
            let prepared = rt.prepared_revoked_goal_tool_round.as_ref().unwrap();
            let expected_messages = match &prepared.results {
                RevokedGoalToolResults::Finalized(messages) => {
                    std::iter::once(prepared.assistant.clone())
                        .chain(messages.iter().cloned())
                        .collect::<Vec<_>>()
                }
                RevokedGoalToolResults::Pending { results, .. } => {
                    assert!(matches!(
                        results.as_slice(),
                        [
                            ToolRoundResult::PendingGoalReport { .. },
                            ToolRoundResult::Finalized(_)
                        ]
                    ));
                    vec![prepared.assistant.clone()]
                }
            };
            let expected_state_time = prepared.settlement.state_updated_at;
            let expected_pending = match &prepared.results {
                RevokedGoalToolResults::Pending { results, .. } => {
                    Some(serde_json::to_value(results).unwrap())
                }
                RevokedGoalToolResults::Finalized(_) => None,
            };
            if pending_report {
                control(&db, &owner, ControlCommand::Release, 2).await;
                let RevokedGoalToolResults::Pending { results, .. } = &rt
                    .prepared_revoked_goal_tool_round
                    .as_ref()
                    .unwrap()
                    .results
                else {
                    panic!("mixed report command was not retained");
                };
                assert_eq!(
                    Some(serde_json::to_value(results).unwrap()),
                    expected_pending
                );
            }
            sqlx::query("DROP TRIGGER fail_revoked_tool_settlement")
                .execute(db.pool())
                .await
                .unwrap();
            rt.retry_terminal_transition().await;
            assert!(rt.terminal_transition_retry.is_none());
            assert!(rt.prepared_revoked_goal_tool_round.is_none());
            let actual_messages: Vec<_> = db
                .get_messages(CONVERSATION)
                .await
                .unwrap()
                .into_iter()
                .filter(|actual| {
                    expected_messages
                        .iter()
                        .any(|expected| actual.message_id == expected.message_id)
                })
                .collect();
            assert_eq!(
                serde_json::to_value(actual_messages).unwrap(),
                serde_json::to_value(expected_messages).unwrap()
            );
            assert_eq!(rt.state_updated_at, expected_state_time);
            assert_eq!(
                db.get_conversation(CONVERSATION)
                    .await
                    .unwrap()
                    .state_updated_at,
                expected_state_time
            );
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
        assert!(rt.active_direct_turn.is_none());
        assert!(rt.pending_direct_turn_terminal.is_none());
        let messages = db.get_messages(CONVERSATION).await.unwrap();
        assert_eq!(messages.len(), if pending_report { 4 } else { 3 });
        let results: Vec<_> = messages
            .iter()
            .filter_map(|message| match &message.content {
                MessageContent::Tool(result) if result.tool_use_id == "inflight-tool" => {
                    Some(result)
                }
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert_eq!(results[0].content, "Cancelled by user");
        if pending_report {
            let reports: Vec<_> = messages
                .iter()
                .filter_map(|message| match &message.content {
                    MessageContent::Tool(content) if content.tool_use_id == "pending-report" => {
                        Some(content)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(reports.len(), 1);
            assert!(reports[0].is_error);
            assert!(reports[0].content.contains("Goal report rejected"));
            let goal = db.conversation_goal(&owner).await.unwrap().unwrap();
            assert_eq!(goal.version.get(), 1);
            assert!(matches!(goal.state, GoalState::Underway));
            assert_eq!(
                db.conversation_goal_history(&owner, GoalVersion::INITIAL)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert!(db
            .load_provider_replay_state(CONVERSATION)
            .await
            .unwrap()
            .is_none());
        let calls = messages
            .iter()
            .filter(|message| {
                matches!(&message.content,
                    MessageContent::Agent(blocks) if blocks.iter().any(|block| matches!(block,
                        ContentBlock::ToolUse { id, .. } if id == "inflight-tool"
                    ))
                )
            })
            .count();
        assert_eq!(calls, 1);
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
        if !pending_report {
            control(&db, &owner, ControlCommand::Release, 2).await;
        }
        assert_eq!(tools.recorded_executions().len(), 1);
    }
}

#[tokio::test]
async fn conversation_goal_stop_release_between_pending_polls_prevents_deferred_external_send() {
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
        let polls = std::sync::atomic::AtomicUsize::new(0);
        let sends = std::sync::atomic::AtomicUsize::new(0);
        let operation = std::future::poll_fn(|_| {
            if polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Poll::Pending
            } else {
                sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Poll::Ready(())
            }
        });
        let mut operation = Box::pin(permit.run(operation));
        std::future::poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        control(&db, &owner, ControlCommand::Stop, 1).await;
        control(&db, &owner, ControlCommand::Release, 2).await;
        assert!(operation.await.is_err());
        assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 0);
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
                        tokio::time::timeout(Duration::from_secs(30), resume.notified())
                            .await
                            .expect("database dispatch test barrier was not resumed");
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
