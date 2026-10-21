use super::*;
use phoenix_core::domain::conversation_goal::{
    GoalRequestLimit, GoalStartInput, GoalStartReceipt, GoalStartRequester,
};
use phoenix_db::conversation_goals::{
    control::{ControlCommand, ControlRequest},
    start::GoalStartAcceptance,
};
use phoenix_db::workflow::ClaimAuthoritativeTurnInput;
use phoenix_workflow::{LeaseExpiry, ProcessIncarnation, WorkflowId};

async fn install(db: Database) -> Database {
    db.install_goal_controls_for_tests()
        .await
        .unwrap()
        .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
        .await
        .unwrap()
}

async fn claimed_start() -> (
    tempfile::TempDir,
    Database,
    GoalStartReceipt,
    phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
) {
    let directory = tempfile::tempdir().unwrap();
    let db = install(
        Database::open(directory.path().join("start.db").to_str().unwrap())
            .await
            .unwrap(),
    )
    .await;
    let root = db
        .create_conversation(CONVERSATION, "runtime Start", "/tmp", true, None, None)
        .await
        .unwrap();
    let owner = root.product_conversation_id;
    let actor = GoalActor::GeneratedHost {
        action_id: text("runtime-start-fixture"),
    };
    let created = db
        .apply_conversation_goal(
            &GoalMutation {
                owner: owner.clone(),
                expected_version: GoalVersion::INITIAL,
                action_id: text("runtime-start-create"),
                attribution: GoalAttribution {
                    requested_by: actor.clone(),
                    applied_by: actor,
                },
                change: GoalChange::Create {
                    goal_id: GoalId::parse("runtime-start-goal").unwrap(),
                    objective: text("prove message-free runtime adoption"),
                },
            },
            CONVERSATION,
            GoalTimestampUs::parse(1).unwrap(),
        )
        .await
        .unwrap();
    change_control(&db, &owner, ControlCommand::Release, 0).await;
    let request = GoalStartInput::new(
        uuid::Uuid::new_v4(),
        GoalStartRequester::Owner {
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
        },
        created.snapshot.fence(),
    )
    .unwrap();
    let accepted = db
        .accept_owner_goal_start(&request, CONVERSATION, GoalTimestampUs::parse(3).unwrap())
        .await
        .unwrap();
    let GoalStartAcceptance::Accepted(receipt) = accepted else {
        panic!("expected fresh Start, got {accepted:?}")
    };
    let now = u64::try_from(Utc::now().timestamp()).unwrap();
    let authority = db
        .workflow_repository()
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: TurnAuthorityId(u64::try_from(receipt.execution().turn_id()).unwrap()),
            workflow_id: WorkflowId(u64::try_from(receipt.execution().workflow_id()).unwrap()),
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(now),
            lease_until: LeaseExpiry(now + 3600),
        })
        .await
        .unwrap()
        .authority
        .unwrap();
    (directory, db, receipt, authority)
}

async fn change_control(
    db: &Database,
    owner: &phoenix_core::domain::product_conversation::ProductConversationId,
    command: ControlCommand,
    version: u64,
) {
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
            request_id: uuid::Uuid::new_v4(),
            command,
            expected_control_version: version,
        },
        GoalTimestampUs::parse(10).unwrap(),
    )
    .await
    .unwrap();
}

async fn ledger(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM conversation_goal_requests),(SELECT count(*) FROM conversation_goal_request_checkpoints),(SELECT count(*) FROM conversation_goal_start_materializations),(SELECT count(*) FROM messages)")
        .fetch_one(db.pool()).await.unwrap()
}

async fn adopted() -> (
    tempfile::TempDir,
    Database,
    GoalStartReceipt,
    TestRuntime,
    Arc<RenderingAnthropicClient>,
) {
    let (directory, db, receipt, authority) = claimed_start().await;
    let client = Arc::new(RenderingAnthropicClient::new());
    let mut rt = runtime(
        db.clone(),
        directory.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    );
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    rt.state = stored.state;
    rt.state_updated_at = stored.state_updated_at;
    let mut result = transition(
        &rt.state,
        &rt.context,
        Event::AuthoritativeGoalStart {
            receipt: receipt.clone(),
            authority,
        },
    )
    .unwrap();
    assert!(result
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::RequestLlm)));
    // Stop at the real executor's adoption boundary so tests can interpose a committed race.
    result
        .effects
        .retain(|effect| !matches!(effect, Effect::RequestLlm));
    rt.apply_transition_result(result).await.unwrap();
    assert_eq!(rt.state, ConvState::LlmRequesting { attempt: 1 });
    let active = rt.active_direct_turn.as_deref().unwrap();
    assert_eq!(
        i64::try_from(active.turn_id.0).unwrap(),
        receipt.execution().turn_id()
    );
    assert_eq!(ledger(&db).await, (1, 0, 1, 0));
    assert!(client.requests.lock().unwrap().is_empty());
    (directory, db, receipt, rt, client)
}

fn assert_start_request(
    client: &RenderingAnthropicClient,
    receipt: &GoalStartReceipt,
    expected_request_id: &str,
) {
    let requests = client.requests.lock().unwrap();
    let request = requests.last().expect("provider actually polled");
    assert_eq!(
        request.telemetry.as_ref().unwrap().request_id,
        expected_request_id
    );
    let system = serde_json::to_string(&request.system).unwrap();
    assert!(system.contains(receipt.observation().objective.as_str()));
    assert!(system.contains("Request purpose: Normal"));
    assert!(request.messages.iter().all(|message| {
        !serde_json::to_string(&message.content)
            .unwrap()
            .contains(receipt.observation().objective.as_str())
    }));
}

async fn request_id(db: &Database) -> String {
    sqlx::query_scalar("SELECT request_id FROM conversation_goal_requests")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn conversation_goal_real_start_event_reaches_provider_without_synthetic_chat() {
    let (directory, db, receipt, authority) = claimed_start().await;
    let client = Arc::new(RenderingAnthropicClient::new());
    let mut rt = runtime(
        db.clone(),
        directory.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    );
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    rt.state = stored.state;
    rt.state_updated_at = stored.state_updated_at;
    rt.process_event(Event::AuthoritativeGoalStart {
        receipt: receipt.clone(),
        authority,
    })
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        rt.llm_task_handle.take().expect("provider task"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(rt.state, ConvState::LlmRequesting { attempt: 1 });
    assert_start_request(&client, &receipt, &request_id(&db).await);
    assert_eq!(ledger(&db).await, (1, 0, 1, 0));
}

#[tokio::test]
async fn conversation_goal_real_executor_adopts_start_and_projects_context_without_chat() {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    let before = ledger(&db).await;
    dispatch(&mut rt).await;
    assert_start_request(&client, &receipt, &request_id(&db).await);
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(ledger(&db).await, before);
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
}

#[tokio::test]
async fn conversation_goal_real_executor_reopen_retry_keeps_request_identity_and_one_charge() {
    let (directory, db, receipt, rt, _client) = adopted().await;
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    let before = ledger(&db).await;
    let request_id: String =
        sqlx::query_scalar("SELECT request_id FROM conversation_goal_requests")
            .fetch_one(db.pool())
            .await
            .unwrap();
    drop(rt);
    db.pool().close().await;
    drop(db);
    let reopened = Database::open(directory.path().join("start.db").to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened.goal_execution_installation(),
        phoenix_db::conversation_goals::execution::GoalExecutionInstallation::Disabled
    );
    let db = install(reopened).await;
    for attempt in [1, 2] {
        if attempt == 2 {
            db.update_conversation_state(CONVERSATION, &ConvState::LlmRequesting { attempt })
                .await
                .unwrap();
        }
        let stored = db.get_conversation(CONVERSATION).await.unwrap();
        let client = Arc::new(RenderingAnthropicClient::new());
        let mut rt = runtime(
            db.clone(),
            directory.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(false)),
        )
        .with_active_direct_turn(Some(active.clone()))
        .with_state_updated_at(stored.state_updated_at);
        rt.state = stored.state;
        dispatch(&mut rt).await;
        assert_start_request(&client, &receipt, &request_id);
        assert_eq!(
            client.requests.lock().unwrap()[0]
                .telemetry
                .as_ref()
                .unwrap()
                .request_id,
            request_id
        );
        assert_eq!(ledger(&db).await, before);
    }
}

#[tokio::test]
async fn conversation_goal_real_executor_summary_cannot_borrow_initial_normal_charge() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    let before = ledger(&db).await;
    let error = rt
        .execute_effect(Effect::RequestContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "start-summary".into(),
                rejected_tool_calls: vec![],
                attempt: 1,
            },
        })
        .await
        .unwrap_err();
    assert!(
        error.contains("purpose requires its own durably admitted intent"),
        "{error}"
    );
    assert!(rt.llm_task_handle.is_none());
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(ledger(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_real_executor_missing_active_start_cannot_fall_back_to_legacy_summary() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    rt.active_direct_turn = None;
    let before = ledger(&db).await;
    let error = rt
        .execute_effect(Effect::RequestContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "missing-active-summary".into(),
                rejected_tool_calls: vec![],
                attempt: 1,
            },
        })
        .await
        .unwrap_err();
    assert!(error.contains("no active runtime execution"), "{error}");
    assert!(rt.llm_task_handle.is_none());
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(ledger(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_request_purpose_keeps_legacy_message_normal_and_summary_paths() {
    let (db, _owner, active) = control_dispatch::fixture(false).await;
    let db = db
        .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let client = Arc::new(RenderingAnthropicClient::new());
    let mut rt = runtime(
        db.clone(),
        directory.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active));
    let before = ledger(&db).await;
    dispatch(&mut rt).await;
    rt.execute_effect(Effect::RequestContinuation {
        request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
            operation_id: "legacy-summary".into(),
            rejected_tool_calls: vec![],
            attempt: 1,
        },
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), rt.llm_task_handle.take().unwrap())
        .await
        .unwrap()
        .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| !serde_json::to_string(&request.system)
            .unwrap()
            .contains("Admitted durable goal")));
    assert_eq!(requests[0].messages.len(), 1);
    drop(requests);
    assert_eq!(ledger(&db).await, before);
    assert_eq!(before.0, 0);
}

async fn complete_bound_goal(db: &Database, receipt: &GoalStartReceipt) {
    let binding = db
        .conversation_goal_execution_binding(receipt.execution())
        .await
        .unwrap()
        .unwrap();
    let actor = GoalActor::OwningAgent(
        source(
            &receipt.input().goal().owner,
            receipt.execution(),
            CONVERSATION,
            "runtime-goal-report",
            "complete",
        )
        .unwrap(),
    );
    db.apply_conversation_goal(
        &GoalMutation {
            owner: receipt.input().goal().owner.clone(),
            expected_version: receipt.input().goal().version,
            action_id: text("runtime-goal-complete"),
            attribution: GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            },
            change: GoalChange::Report {
                binding,
                disposition: phoenix_core::domain::conversation_goal::GoalDisposition::Complete(
                    text("done"),
                ),
            },
        },
        CONVERSATION,
        GoalTimestampUs::parse(11).unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        db.conversation_goal(&receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete(_)
    ));
}

#[derive(Clone, Copy, Debug)]
enum Revocation {
    Stop,
    ReleaseAba,
    Close,
    History,
    StaleLatest,
    Complete,
    Replacement,
    QueuedInput,
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_real_executor_rechecks_committed_live_fences_before_request() {
    for reason in [
        Revocation::Stop,
        Revocation::ReleaseAba,
        Revocation::Close,
        Revocation::History,
        Revocation::StaleLatest,
        Revocation::Complete,
        Revocation::Replacement,
        Revocation::QueuedInput,
    ] {
        let (_directory, db, receipt, mut rt, client) = adopted().await;
        match reason {
            Revocation::Stop | Revocation::ReleaseAba => {
                change_control(&db, &receipt.input().goal().owner, ControlCommand::Stop, 1).await;
                if matches!(reason, Revocation::ReleaseAba) {
                    change_control(
                        &db,
                        &receipt.input().goal().owner,
                        ControlCommand::Release,
                        2,
                    )
                    .await;
                }
            }
            Revocation::Close => {
                db.begin_close_foundation(
                    &receipt.input().goal().owner,
                    &phoenix_core::domain::close::TranscriptConversationId::parse(CONVERSATION)
                        .unwrap(),
                    "start-dispatch-close",
                )
                .await
                .unwrap();
            }
            Revocation::History => {
                sqlx::query(
                    "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=?",
                )
                .bind(receipt.input().goal().owner.as_str())
                .execute(db.pool())
                .await
                .unwrap();
            }
            Revocation::Complete | Revocation::Replacement => {
                complete_bound_goal(&db, &receipt).await;
                if matches!(reason, Revocation::Replacement) {
                    let stale_active = rt.active_direct_turn.as_deref().unwrap().clone();
                    rt.state = ConvState::Idle;
                    rt.pending_direct_turn_terminal = Some(Box::new(
                        crate::runtime::traits::ActiveDirectTurnTerminal::Completed,
                    ));
                    let mut admitted = rt.admit_authoritative_effect().unwrap();
                    rt.persist_state_effect(false, &mut admitted).await.unwrap();
                    let actor = GoalActor::GeneratedHost {
                        action_id: text("replacement"),
                    };
                    db.apply_conversation_goal(
                        &GoalMutation {
                            owner: receipt.input().goal().owner.clone(),
                            expected_version: db
                                .conversation_goal(&receipt.input().goal().owner)
                                .await
                                .unwrap()
                                .unwrap()
                                .version,
                            action_id: text("replace-completed-runtime-goal"),
                            attribution: GoalAttribution {
                                requested_by: actor.clone(),
                                applied_by: actor,
                            },
                            change: GoalChange::Create {
                                goal_id: GoalId::parse("replacement-runtime-goal").unwrap(),
                                objective: text("a replacement must not revive the old request"),
                            },
                        },
                        CONVERSATION,
                        GoalTimestampUs::parse(12).unwrap(),
                    )
                    .await
                    .unwrap();
                    rt.active_direct_turn = Some(Box::new(stale_active));
                    rt.state = ConvState::LlmRequesting { attempt: 1 };
                }
            }
            Revocation::QueuedInput => {
                db.append_steering_entry(
                    CONVERSATION,
                    &phoenix_core::domain::sm_event::SteerEntry {
                        text: "accepted input has priority".into(),
                        origin: InputOrigin::UnknownHistorical,
                        llm_text: None,
                        images: vec![],
                        files: vec![],
                        message_id: "queued-before-start-send".into(),
                        user_agent: None,
                        skill_invocation: None,
                    },
                    "start-queue-fingerprint",
                )
                .await
                .unwrap();
            }
            Revocation::StaleLatest => {
                db.update_conversation_state(
                    CONVERSATION,
                    &ConvState::ContextExhausted {
                        summary: "continue this goal".into(),
                    },
                )
                .await
                .unwrap();
                assert!(matches!(
                    db.continue_conversation(CONVERSATION).await.unwrap(),
                    phoenix_db::ContinueOutcome::Created(_)
                ));
            }
        }
        let before = ledger(&db).await;
        let error = rt.execute_effect(Effect::RequestLlm).await.unwrap_err();
        assert!(!error.is_empty(), "{reason:?}");
        if matches!(reason, Revocation::QueuedInput) {
            assert!(error.contains("accepted queued input"), "{error}");
            assert_eq!(db.steering_queue_depth(CONVERSATION).await.unwrap(), 1);
        }
        if matches!(reason, Revocation::Complete) {
            assert!(error.contains("exact live Underway binding"), "{error}");
        }
        assert!(rt.llm_task_handle.is_none(), "{reason:?}");
        assert!(client.requests.lock().unwrap().is_empty(), "{reason:?}");
        assert_eq!(ledger(&db).await, before, "{reason:?}");
    }
}
