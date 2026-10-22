#[path = "checkpoints/coordinator.rs"]
mod coordinator;

use super::*;
use crate::runtime::executor::goal_request_checkpoint::LlmCompletion as GoalRequestCompletion;

fn provider_response(content: Vec<ContentBlock>) -> phoenix_core::domain::llm_types::LlmResponse {
    phoenix_core::domain::llm_types::LlmResponse::non_streaming(
        content,
        true,
        phoenix_core::domain::llm_types::Usage::default(),
    )
}

fn provider_response_with_replay(
    response_id: &str,
    content: Vec<ContentBlock>,
) -> phoenix_core::domain::llm_types::LlmResponse {
    use phoenix_core::domain::provider_replay::{AnthropicResponseIdentity, AnthropicResponseSet};
    let replay = AnthropicResponseSet::with_public_content(
        AnthropicResponseIdentity {
            response_id: response_id.into(),
            model: "claude-opus-5-5".into(),
        },
        content.clone(),
        Vec::new(),
    )
    .unwrap();
    let mut response = provider_response(content);
    response.provider_replay = Some(ProviderReplayUpdate::Anthropic(replay));
    response
}

pub(super) async fn dispatch_response<T: ToolExecutor + 'static>(
    rt: &mut ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, T>,
) -> GoalRequestCompletion {
    rt.execute_effect(Effect::RequestLlm).await.unwrap();
    rt.llm_task_handle.as_mut().unwrap().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), rt.llm_outcome_rx.recv())
        .await
        .expect("actual provider completion liveness")
        .expect("receipt owner remains open")
}

fn actual_tool_runtime(
    initial: &mut TestRuntime,
    db: &Database,
    client: Arc<RenderingAnthropicClient>,
) -> ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, MockToolExecutor> {
    let (event_tx, event_rx) = mpsc::channel(8);
    ConversationRuntime::new(
        initial.context.clone(),
        initial.state.clone(),
        DatabaseStorage::new(db.clone()),
        client,
        MockToolExecutor::new().with_tool("think", ToolOutput::success("actual tool output")),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx,
        SseBroadcaster::new(32, 4),
    )
    .with_state_updated_at(initial.state_updated_at)
    .with_active_direct_turn(initial.active_direct_turn.take().map(|active| *active))
}

pub(super) async fn receive_automatic_response<T: ToolExecutor + 'static>(
    rt: &mut ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, T>,
) -> GoalRequestCompletion {
    rt.llm_task_handle
        .as_mut()
        .expect("existing producer dispatched the next request")
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), rt.llm_outcome_rx.recv())
        .await
        .expect("ordinary completion-channel liveness")
        .unwrap()
}

async fn process_prepared_report<T: ToolExecutor + 'static>(
    rt: &mut ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, T>,
) {
    let event = rt
        .event_rx
        .try_recv()
        .expect("goal_report queued its prepared event");
    assert!(matches!(event, Event::GoalReportPrepared { .. }));
    rt.process_event(event).await.unwrap();
}

pub(super) async fn guard_terminal_projection_order(db: &Database) {
    sqlx::query(
        "CREATE TRIGGER goal_idle_requires_settled_owner
         BEFORE UPDATE OF state ON conversations
         WHEN json_extract(NEW.state, '$.type')='idle'
         BEGIN
           SELECT CASE WHEN EXISTS(SELECT 1 FROM durable_turns
             WHERE conversation_id=NEW.id AND owns_conversation=1 AND terminal_kind IS NULL)
           THEN RAISE(ABORT, 'Idle preceded actual terminal-owner settlement') END;
         END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER goal_terminal_requires_obligation
         BEFORE UPDATE OF terminal_kind ON durable_turns
         WHEN NEW.input_kind='goal_start' AND NEW.terminal_kind='Completed'
         BEGIN
           SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM direct_turn_terminal_obligations
             WHERE turn_id=NEW.turn_id AND expected_generation=OLD.generation)
           THEN RAISE(ABORT, 'terminal settlement omitted its existing obligation') END;
         END",
    )
    .execute(db.pool())
    .await
    .unwrap();
}

pub(super) async fn assert_completed_start_owner(db: &Database, turn_id: u64, generation: u64) {
    let (terminal, owns, current): (Option<String>, bool, i64) = sqlx::query_as(
        "SELECT terminal_kind, owns_conversation, generation FROM durable_turns WHERE turn_id=?",
    )
    .bind(i64::try_from(turn_id).unwrap())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(terminal.as_deref(), Some("Completed"));
    assert!(!owns);
    assert_eq!(u64::try_from(current).unwrap(), generation + 1);
    let state: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id=?")
        .bind(CONVERSATION)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ConvState>(&state).unwrap(),
        ConvState::Idle
    );
}

#[tokio::test]
async fn conversation_goal_actual_no_tool_yield_automatically_reaches_explicit_terminal_report_in_same_execution(
) {
    use phoenix_core::domain::conversation_goal::{GoalReportOutcome, GoalState};
    for outcome in [
        GoalReportOutcome::Complete(text("finished")),
        GoalReportOutcome::Blocked(text("needs external input")),
    ] {
        let (_directory, db, receipt, mut initial, client) = adopted().await;
        let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
        let execution = rt.active_direct_turn.as_deref().unwrap().clone();
        guard_terminal_projection_order(&db).await;
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
        rt.state_watcher = Some(watch_tx);
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response_with_replay(
                "automatic-first",
                vec![ContentBlock::text(
                    "ordinary yield, not a Goal progress report",
                )],
            ));
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::tool_use(
                "explicit-final-report",
                "goal_report",
                serde_json::to_value(&outcome).unwrap(),
            )]));
        let first = dispatch_response(&mut rt).await;
        let first_id = first.admitted.as_ref().unwrap().intent().id.clone();
        rt.process_llm_completion(first).await.unwrap();
        let after_yield = db
            .conversation_goal(&receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_yield.version, receipt.input().goal().version);
        assert!(matches!(after_yield.state, GoalState::Underway));
        assert!(matches!(
            &*watch_rx.borrow(),
            ConvState::LlmRequesting { .. }
        ));
        let obligations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM direct_turn_terminal_obligations")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            obligations, 0,
            "yield retains its baton without a terminal obligation"
        );
        let second = receive_automatic_response(&mut rt).await;
        let second_receipt = second
            .admitted
            .as_ref()
            .expect("same-execution automatic request is accounted");
        assert_ne!(second_receipt.intent().id, first_id);
        assert_eq!(second_receipt.intent().execution, receipt.execution());
        assert_eq!(
            second_receipt.intent().goal,
            *receipt.input().goal(),
            "unreported Underway yield preserves the exact immutable admission fence"
        );
        assert_eq!(
            rt.active_direct_turn.as_deref().unwrap().turn_id,
            execution.turn_id
        );
        assert_eq!(
            rt.active_direct_turn.as_deref().unwrap().generation,
            execution.generation
        );
        rt.process_llm_completion(second).await.unwrap();
        process_prepared_report(&mut rt).await;
        assert!(
            rt.tool_outcome_rx.try_recv().is_err(),
            "goal_report queues the prepared event rather than an async tool outcome"
        );
        let goal = db
            .conversation_goal(&receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(goal.version.get(), receipt.input().goal().version.get() + 1);
        match outcome {
            GoalReportOutcome::Complete(_) => assert!(matches!(goal.state, GoalState::Complete(_))),
            GoalReportOutcome::Blocked(_) => assert!(matches!(goal.state, GoalState::Blocked(_))),
            _ => unreachable!(),
        }
        assert_eq!(rt.state, ConvState::Idle);
        assert_eq!(*watch_rx.borrow(), ConvState::Idle);
        assert!(rt.active_direct_turn.is_none());
        assert!(rt.pending_direct_turn_terminal.is_none());
        assert_completed_start_owner(&db, execution.turn_id.0, execution.generation).await;
        assert!(rt.llm_task_handle.is_none());
        assert_eq!(
            client.requests.lock().unwrap().len(),
            2,
            "last-allowance report must not dispatch a third request or be clobbered by Exhausted"
        );
        let (requests, checkpoints, executions, messages) = ledger(&db).await;
        assert_eq!((requests, checkpoints, executions, messages), (2, 2, 1, 3));
        assert!(db
            .get_messages(CONVERSATION)
            .await
            .unwrap()
            .iter()
            .all(|message| message.message_type != crate::db::MessageType::User));
    }
}

#[tokio::test]
async fn conversation_goal_actual_blocked_resume_requires_new_start_not_version_refresh() {
    use phoenix_core::domain::conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalDisposition, GoalMutation,
        GoalRequestOperation, GoalState,
    };
    let (_directory, db, receipt, mut initial, client) = adopted_with_limit(3).await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    let old = rt.active_direct_turn.as_deref().unwrap().clone();
    let old_stamp = rt.state_updated_at;
    let old_binding = db
        .conversation_goal_execution_binding(receipt.execution())
        .await
        .unwrap()
        .unwrap();
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::ToolUse {
            id: "explicit-blocked-before-resume".into(),
            name: "goal_report".into(),
            input: serde_json::json!({"status":"blocked", "reason":"need user input"}),
        }]));
    let response = dispatch_response(&mut rt).await;
    rt.process_llm_completion(response).await.unwrap();
    process_prepared_report(&mut rt).await;
    assert_eq!(rt.state, ConvState::Idle);
    assert!(rt.active_direct_turn.is_none());
    let blocked = db
        .conversation_goal(&receipt.input().goal().owner)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(blocked.state, GoalState::Blocked(_)));
    assert_eq!(
        blocked.version.get(),
        receipt.input().goal().version.get() + 1
    );
    let actor = GoalActor::GeneratedHost {
        action_id: text("explicit-user-authorized-resume"),
    };
    let resumed = db
        .apply_conversation_goal(
            &GoalMutation {
                owner: blocked.owner.clone(),
                expected_version: blocked.version,
                action_id: text("explicit-user-authorized-resume"),
                attribution: GoalAttribution {
                    requested_by: actor.clone(),
                    applied_by: actor,
                },
                change: GoalChange::SetDisposition {
                    goal_id: blocked.goal_id.clone(),
                    disposition: GoalDisposition::Underway,
                },
            },
            CONVERSATION,
            GoalTimestampUs::parse(30).unwrap(),
        )
        .await
        .unwrap()
        .snapshot;
    assert_eq!(resumed.goal_id, blocked.goal_id);
    assert_eq!(resumed.version.get(), blocked.version.get() + 1);
    assert_eq!(resumed.state, GoalState::Underway);
    let before = ledger(&db).await;
    let repo = db.workflow_repository();
    assert!(repo
        .admit_goal_start_request_context(
            CONVERSATION,
            Some((old.turn_id, old.generation)),
            old_stamp,
            GoalRequestOperation::Normal { attempt: 1 }
        )
        .await
        .is_err());
    assert!(matches!(
        repo.recover_goal_start_runtime(&phoenix_db::workflow::GoalStartRecoveryInput::new(
            CONVERSATION.into(),
            (old.turn_id, old.generation),
            (ConvState::LlmRequesting { attempt: 1 }, old_stamp),
            Timestamp(31),
        ))
        .await
        .unwrap(),
        phoenix_db::workflow::GoalStartRecoveryOutcome::Refused
    ));
    assert_eq!(
        ledger(&db).await,
        before,
        "resuming facts never starts or refreshes the old execution"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(
        db.conversation_goal_execution_binding(receipt.execution())
            .await
            .unwrap()
            .unwrap(),
        old_binding
    );
    let start = GoalStartInput::new(
        uuid::Uuid::new_v4(),
        GoalStartRequester::Owner {
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
        },
        resumed.fence(),
    )
    .unwrap();
    let GoalStartAcceptance::Accepted(new_receipt) = db
        .accept_owner_goal_start(&start, CONVERSATION, GoalTimestampUs::parse(32).unwrap())
        .await
        .unwrap()
    else {
        panic!("new explicit Start is required")
    };
    assert_ne!(new_receipt.execution(), receipt.execution());
    assert_eq!(new_receipt.input().goal(), &resumed.fence());
    assert_eq!(
        ledger(&db).await,
        before,
        "typed Start acceptance is not a request charge or provider send"
    );
    let workflow =
        phoenix_workflow::WorkflowId(u64::try_from(new_receipt.execution().workflow_id()).unwrap());
    let now = u64::try_from(Utc::now().timestamp()).unwrap();
    let claim = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: TurnAuthorityId(u64::try_from(new_receipt.execution().turn_id()).unwrap()),
            workflow_id: workflow,
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(now),
            lease_until: LeaseExpiry(now + 3600),
        })
        .await
        .unwrap()
        .authority
        .unwrap();
    let authority = phoenix_core::domain::sm_event::DirectTurnAttemptAuthority::new(
        claim.workflow_id.0,
        u64::try_from(new_receipt.execution().turn_id()).unwrap(),
        claim.effect_id.0,
        claim.attempt_id.0,
        claim.declared_workflow_version.0,
        claim.generation.0,
        claim.process_incarnation.0,
    );
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    rt.state = stored.state;
    rt.state_updated_at = stored.state_updated_at;
    let mut transition = transition(
        &rt.state,
        &rt.context,
        Event::AuthoritativeGoalStart {
            receipt: new_receipt.clone(),
            authority,
        },
    )
    .unwrap();
    transition
        .effects
        .retain(|effect| !matches!(effect, Effect::RequestLlm));
    rt.apply_transition_result(transition).await.unwrap();
    let active = rt.active_direct_turn.as_deref().unwrap();
    let context = repo
        .admit_goal_start_request_context(
            CONVERSATION,
            Some((active.turn_id, active.generation)),
            rt.state_updated_at,
            GoalRequestOperation::Normal { attempt: 1 },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context.admitted().intent().goal, resumed.fence());
    assert_eq!(
        db.conversation_goal_execution_binding(new_receipt.execution())
            .await
            .unwrap()
            .unwrap()
            .observation,
        phoenix_core::domain::conversation_goal::GoalAdmissionObservation::Goal(resumed.fence())
    );
    assert_eq!(
        db.conversation_goal_execution_binding(receipt.execution())
            .await
            .unwrap()
            .unwrap(),
        old_binding
    );
    assert_eq!(
        ledger(&db).await,
        (before.0 + 1, before.1, before.2 + 1, before.3),
        "new binding uses the same finite per-goal ledger, not a refill"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn conversation_goal_actual_automatic_yields_exhaust_once_without_inferred_progress_or_refill(
) {
    use phoenix_core::domain::conversation_goal::GoalState;
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    let execution = rt.active_direct_turn.as_deref().unwrap().clone();
    guard_terminal_projection_order(&db).await;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    for answer in ["first ordinary answer", "second ordinary answer"] {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::text(answer)]));
    }
    let first = dispatch_response(&mut rt).await;
    rt.process_llm_completion(first).await.unwrap();
    let second = receive_automatic_response(&mut rt).await;
    rt.process_llm_completion(second).await.unwrap();
    let goal = db
        .conversation_goal(&receipt.input().goal().owner)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(goal.state, GoalState::Exhausted(_)));
    assert_eq!(goal.version.get(), receipt.input().goal().version.get() + 1);
    assert_eq!(rt.state, ConvState::Idle);
    assert_eq!(*watch_rx.borrow(), ConvState::Idle);
    assert!(rt.active_direct_turn.is_none());
    assert!(rt.pending_direct_turn_terminal.is_none());
    assert_completed_start_owner(&db, execution.turn_id.0, execution.generation).await;
    assert!(rt.llm_task_handle.is_none());
    assert_eq!(ledger(&db).await, (2, 2, 1, 2));
    let new_start = GoalStartInput::new(
        uuid::Uuid::new_v4(),
        GoalStartRequester::Owner {
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
        },
        goal.fence(),
    )
    .unwrap();
    assert!(matches!(
        db.accept_owner_goal_start(
            &new_start,
            CONVERSATION,
            GoalTimestampUs::parse(20).unwrap()
        )
        .await,
        Err(phoenix_db::conversation_goals::start::GoalStartError::StaleGoal)
    ));
    assert_eq!(ledger(&db).await, (2, 2, 1, 2));
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    change_control(&db, &receipt.input().goal().owner, ControlCommand::Stop, 1).await;
    change_control(
        &db,
        &receipt.input().goal().owner,
        ControlCommand::Release,
        2,
    )
    .await;
    assert!(matches!(
        db.accept_owner_goal_start(
            &new_start,
            CONVERSATION,
            GoalTimestampUs::parse(21).unwrap()
        )
        .await,
        Err(phoenix_db::conversation_goals::start::GoalStartError::StaleGoal)
    ));
    assert_eq!(
        ledger(&db).await,
        (2, 2, 1, 2),
        "Stop/release never refills an immutable goal's allowance"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    assert!(matches!(
        db.conversation_goal(&receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Exhausted(_)
    ));
}

#[tokio::test]
async fn conversation_goal_ephemeral_tokens_use_ordinary_stream_before_canonical_commit_without_goal_mutation(
) {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    *client.streamed_text.lock().unwrap() = Some("ordinary streamed text".into());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "canonical answer",
        )]));
    let before = canonical_counts(&db).await;
    let mut events = rt.broadcast_tx.subscribe();
    let completion = dispatch_response(&mut rt).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let SseEvent::Token { text, .. } = events.recv().await.unwrap() {
                assert_eq!(text, "ordinary streamed text");
                break;
            }
        }
    })
    .await
    .expect("actual token forwarder publication");
    assert_eq!(canonical_counts(&db).await, before);
    let goal = db
        .conversation_goal(&receipt.input().goal().owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(goal.version, receipt.input().goal().version);
    assert!(matches!(
        goal.state,
        phoenix_core::domain::conversation_goal::GoalState::Underway
    ));
    rt.process_llm_completion(completion).await.unwrap();
    assert_eq!(canonical_counts(&db).await.2, 1);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_automatic_logical_boundary_refreshes_guidance_and_skill_metadata_without_new_execution(
) {
    let (directory, _db, receipt, mut rt, client) = adopted().await;
    let skill_dir = directory.path().join(".claude/skills/goal-boundary");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill = skill_dir.join("SKILL.md");
    std::fs::write(
        directory.path().join("AGENTS.md"),
        "goal guidance before logical yield",
    )
    .unwrap();
    std::fs::write(&skill, "---\nname: goal-boundary\ndescription: metadata before logical yield\n---\nUse current instructions.\n").unwrap();
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text("yield")]));
    let first = dispatch_response(&mut rt).await;
    std::fs::write(
        directory.path().join("AGENTS.md"),
        "goal guidance after logical yield",
    )
    .unwrap();
    std::fs::write(&skill, "---\nname: goal-boundary\ndescription: metadata after logical yield\n---\nUse current instructions.\n").unwrap();
    rt.process_llm_completion(first).await.unwrap();
    let second = receive_automatic_response(&mut rt).await;
    assert_eq!(
        second.admitted.as_ref().unwrap().intent().execution,
        receipt.execution()
    );
    let requests = client.requests.lock().unwrap();
    let before = requests[0]
        .system
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let after = requests[1]
        .system
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(before.contains("goal guidance before logical yield"));
    assert!(before.contains("metadata before logical yield"));
    assert!(after.contains("goal guidance after logical yield"));
    assert!(after.contains("metadata after logical yield"));
    assert!(!after.contains("metadata before logical yield"));
    drop(requests);
    rt.abort_external_effects_in_memory();
}

async fn canonical_counts(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM messages),
        (SELECT count(*) FROM conversation_goal_requests),
        (SELECT count(*) FROM conversation_goal_request_checkpoints),
        (SELECT count(*) FROM direct_turn_terminal_obligations)",
    )
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn conversation_goal_actual_provider_response_commits_checkpoint_and_keeps_same_execution() {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "actual canonical answer",
        )]));
    let before = canonical_counts(&db).await;
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    let completion = dispatch_response(&mut rt).await;
    let LlmOutcome::Response {
        content,
        provider_replay,
        tool_calls,
        end_turn,
        usage,
        request_id,
    } = &completion.outcome
    else {
        panic!("scripted provider must produce its actual response");
    };
    let duplicate = LlmOutcome::Response {
        content: content.clone(),
        provider_replay: provider_replay.clone(),
        tool_calls: tool_calls.clone(),
        end_turn: *end_turn,
        usage: usage.clone(),
        request_id: request_id.clone(),
    };
    let request_id = completion
        .admitted
        .as_ref()
        .unwrap()
        .intent()
        .id
        .as_str()
        .to_owned();
    assert_eq!(
        canonical_counts(&db).await,
        before,
        "dispatch is not a checkpoint"
    );
    let (watch_tx, mut watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    rt.process_llm_completion(completion).await.unwrap();
    let after = canonical_counts(&db).await;
    assert_eq!(after, (before.0 + 1, before.1 + 1, before.2 + 1, before.3));
    let message = db.get_message_by_id(&request_id).await.unwrap();
    assert!(matches!(message.content, MessageContent::Agent(_)));
    assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    assert!(watch_rx.has_changed().unwrap());
    assert!(matches!(
        *watch_rx.borrow_and_update(),
        ConvState::LlmRequesting { .. }
    ));
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (rt.state.clone(), rt.state_updated_at)
    );
    assert_eq!(persisted.message_count, 1);
    assert_eq!(rt.active_direct_turn.as_deref(), Some(&active));
    let (owns, terminal): (i64, Option<String>) =
        sqlx::query_as("SELECT owns_conversation,terminal_kind FROM durable_turns WHERE turn_id=?")
            .bind(receipt.execution().turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!((owns, terminal), (1, None));
    let mut publications = rt.broadcast_tx.subscribe();
    assert!(rt
        .process_outcome(EffectOutcome::Llm(duplicate))
        .await
        .is_err());
    assert!(publications.try_recv().is_err());
    assert_eq!(canonical_counts(&db).await, after);
    assert!(!watch_rx.has_changed().unwrap());
    let successor = receive_automatic_response(&mut rt).await;
    assert_eq!(
        successor.admitted.as_ref().unwrap().intent().execution,
        receipt.execution()
    );
    assert_eq!(
        client.requests.lock().unwrap().len(),
        2,
        "the existing producer dispatches the admitted same-execution successor"
    );
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_actual_tool_completion_checkpoints_persisted_tool_source_before_next_request(
) {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response_with_replay(
            "tool-round-response",
            vec![ContentBlock::tool_use(
                "actual-think-call",
                "think",
                serde_json::json!({"thoughts":"checkpoint the actual round"}),
            )],
        ));
    let completion = dispatch_response(&mut rt).await;
    let first_request = completion
        .admitted
        .as_ref()
        .unwrap()
        .intent()
        .id
        .as_str()
        .to_owned();
    rt.process_llm_completion(completion).await.unwrap();
    assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (rt.state.clone(), rt.state_updated_at)
    );
    let before = canonical_counts(&db).await;
    assert_eq!(before.2, 0);
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(
        db.load_provider_replay_state(CONVERSATION)
            .await
            .unwrap()
            .is_none(),
        "real provider replay must not persist with intermediate ToolExecuting state"
    );
    let (generation, outcome) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rt.tool_outcome_rx.recv(),
    )
    .await
    .expect("actual tool completion liveness")
    .unwrap();
    assert!(!rt.tool_outcome_is_stale(generation));
    rt.process_outcome(EffectOutcome::Tool(outcome))
        .await
        .unwrap();
    let counts = canonical_counts(&db).await;
    assert_eq!(counts.0, before.0 + 2);
    assert_eq!(counts.2, 1);
    assert_eq!(counts.3, 0);
    assert!(rt.pending_goal_request_checkpoint.is_none());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_some());
    let checkpointed: String =
        sqlx::query_scalar("SELECT request_id FROM conversation_goal_request_checkpoints")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(checkpointed, first_request);
    let next: (i64, String) = sqlx::query_as("SELECT ordinal,predecessor_request_id FROM conversation_goal_requests ORDER BY ordinal DESC LIMIT 1")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(next, (2, first_request));
    rt.llm_task_handle.take().unwrap().await.unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].provider_replay.is_some(),
        "successor observes the committed replay bundle"
    );
}

#[tokio::test]
async fn conversation_goal_late_actual_response_after_stop_input_slot_or_reconstruction_has_zero_effects(
) {
    for fence in ["stop", "queue", "slot", "reconstruction"] {
        let (_directory, db, receipt, mut rt, client) = adopted().await;
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::text(
                "obsolete answer",
            )]));
        let completion = dispatch_response(&mut rt).await;
        match fence {
            "stop" => {
                use phoenix_db::conversation_goals::control::{ControlCommand, ControlRequest};
                db.apply_goal_control(
                    &ControlRequest {
                        owner: receipt.input().goal().owner.clone(),
                        principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
                        request_id: uuid::Uuid::new_v4(),
                        command: ControlCommand::Stop,
                        expected_control_version: 1,
                    },
                    GoalTimestampUs::parse(100).unwrap(),
                )
                .await
                .unwrap();
            }
            "queue" => {
                let entry = phoenix_core::domain::sm_event::SteerEntry {
                    message_id: "completion-race-input".into(),
                    text: "accepted input wins".into(),
                    origin: InputOrigin::UnknownHistorical,
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: None,
                    skill_invocation: None,
                };
                db.append_steering_entry(CONVERSATION, &entry, "completion-race-fingerprint")
                    .await
                    .unwrap();
            }
            "slot" => {
                db.update_conversation_state_at(
                    CONVERSATION,
                    &rt.state,
                    rt.state_updated_at + chrono::Duration::microseconds(1),
                )
                .await
                .unwrap();
            }
            "reconstruction" => {
                let active = rt.active_direct_turn.as_deref().unwrap();
                assert!(matches!(
                    db.workflow_repository()
                        .recover_goal_start_runtime(
                            &phoenix_db::workflow::GoalStartRecoveryInput::new(
                                CONVERSATION.to_owned(),
                                (active.turn_id, active.generation),
                                (rt.state.clone(), rt.state_updated_at),
                                Timestamp(100),
                            )
                        )
                        .await
                        .unwrap(),
                    phoenix_db::workflow::GoalStartRecoveryOutcome::Recovered(_)
                ));
            }
            _ => unreachable!(),
        }
        let before = canonical_counts(&db).await;
        let mut publications = rt.broadcast_tx.subscribe();
        let handle = rt.llm_task_handle.as_ref().unwrap().id();
        rt.process_llm_completion(completion).await.unwrap();
        assert_eq!(rt.llm_task_handle.as_ref().unwrap().id(), handle, "{fence}");
        assert!(
            matches!(
                db.conversation_goal(&receipt.input().goal().owner)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                phoenix_core::domain::conversation_goal::GoalState::Underway
            ),
            "{fence}: stale/blocked admission is not allowance exhaustion"
        );
        assert!(publications.try_recv().is_err(), "{fence}");
        assert_eq!(canonical_counts(&db).await, before, "{fence}");
    }
}

#[tokio::test]
async fn conversation_goal_summary_completion_cannot_fallback_to_ordinary_canonical_state_write() {
    let (_directory, db, _receipt, mut rt, _client) = adopted().await;
    let before = db.get_conversation(CONVERSATION).await.unwrap();
    let counts = canonical_counts(&db).await;
    let mut publications = rt.broadcast_tx.subscribe();
    let error = rt
        .process_event(Event::ContinuationResponse {
            operation_id: "unowned-summary".into(),
            summary: "must not write this as ordinary continuation".into(),
        })
        .await
        .unwrap_err();
    assert!(error.contains("Summary completion is held"), "{error}");
    let after = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (after.state, after.state_updated_at),
        (before.state, before.state_updated_at)
    );
    assert_eq!(canonical_counts(&db).await, counts);
    assert!(publications.try_recv().is_err());
}

#[tokio::test]
async fn conversation_goal_two_actual_response_operations_cannot_consume_each_others_receipt() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    for _ in 0..2 {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::text(
                "same logical retry answer",
            )]));
    }
    let old = dispatch_response(&mut rt).await;
    let current = dispatch_response(&mut rt).await;
    assert_eq!(
        old.admitted.as_ref().unwrap().intent().id,
        current.admitted.as_ref().unwrap().intent().id
    );
    assert_ne!(old.generation, current.generation);
    let before = canonical_counts(&db).await;
    let mut publications = rt.broadcast_tx.subscribe();
    rt.process_llm_completion(old).await.unwrap();
    assert_eq!(canonical_counts(&db).await, before);
    assert!(publications.try_recv().is_err());
    assert!(rt.pending_goal_request_checkpoint.is_none());
    rt.process_llm_completion(current).await.unwrap();
    assert_eq!(
        canonical_counts(&db).await,
        (before.0 + 1, before.1 + 1, before.2 + 1, before.3)
    );
    let successor = receive_automatic_response(&mut rt).await;
    assert!(successor.admitted.is_some());
    assert_eq!(client.requests.lock().unwrap().len(), 3);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_actual_report_round_commits_mutation_and_checkpoint_once_then_holds_next_request(
) {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::tool_use(
            "actual-report-complete",
            "goal_report",
            serde_json::json!({"status":"complete","reason":"actual admitted report"}),
        )]));
    let before = canonical_counts(&db).await;
    let completion = dispatch_response(&mut rt).await;
    let request_id = completion
        .admitted
        .as_ref()
        .unwrap()
        .intent()
        .id
        .as_str()
        .to_owned();
    rt.process_llm_completion(completion).await.unwrap();
    process_prepared_report(&mut rt).await;
    assert_eq!(rt.state, ConvState::Idle);
    assert!(rt.active_direct_turn.is_none());
    assert!(rt.pending_direct_turn_terminal.is_none());
    let after = canonical_counts(&db).await;
    assert_eq!(after, (before.0 + 2, before.1, before.2 + 1, before.3 + 1));
    let goal = db
        .conversation_goal(&receipt.input().goal().owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(goal.version.get(), receipt.input().goal().version.get() + 1);
    assert!(matches!(
        goal.state,
        phoenix_core::domain::conversation_goal::GoalState::Complete(_)
    ));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    let assistant = messages
        .iter()
        .find(|message| message.message_id == request_id)
        .unwrap();
    let MessageContent::Agent(content) = &assistant.content else {
        panic!("canonical assistant required");
    };
    assert!(content.iter().any(|block| matches!(block,
        ContentBlock::ToolUse { id, .. } if id == "actual-report-complete")));
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message.content, MessageContent::Tool(_)))
            .count(),
        1
    );
    assert!(rt.pending_goal_request_checkpoint.is_none());
    assert_eq!(canonical_counts(&db).await, after);
}

#[tokio::test]
async fn conversation_goal_actual_tool_round_rollback_keeps_exact_source_and_no_publication_or_successor(
) {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response_with_replay(
            "rollback-tool-response",
            vec![ContentBlock::tool_use(
                "rollback-think",
                "think",
                serde_json::json!({"thoughts":"whole round or nothing"}),
            )],
        ));
    let current = dispatch_response(&mut rt).await;
    rt.process_llm_completion(current).await.unwrap();
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    assert!(matches!(source, ConvState::ToolExecuting { .. }));
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    let before = canonical_counts(&db).await;
    let (generation, outcome) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rt.tool_outcome_rx.recv(),
    )
    .await
    .expect("real tool completion liveness")
    .unwrap();
    assert!(!rt.tool_outcome_is_stale(generation));
    sqlx::raw_sql(
        "CREATE TRIGGER reject_actual_tool_checkpoint BEFORE UPDATE OF state ON conversations
        BEGIN SELECT RAISE(ABORT,'injected actual tool checkpoint state failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(rt
        .process_outcome(EffectOutcome::Tool(outcome))
        .await
        .is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (stored.state, stored.state_updated_at),
        (source.clone(), source_stamp)
    );
    let pending = rt.pending_goal_request_checkpoint.as_ref().unwrap();
    assert_eq!(pending.source, source);
    assert_eq!(pending.source_updated_at, source_stamp);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_none());
    assert!(publications.try_recv().is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_stale_same_id_completion_preserves_newer_handle_and_checkpoint_receipt()
{
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    for _ in 0..3 {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::tool_use(
                "same-slot-think",
                "think",
                serde_json::json!({"thoughts":"real owned round"}),
            )]));
    }
    let mut old_error = dispatch_response(&mut rt).await;
    let old_response = dispatch_response(&mut rt).await;
    let current = dispatch_response(&mut rt).await;
    assert_eq!(
        old_error.admitted.as_ref().unwrap().intent().id,
        current.admitted.as_ref().unwrap().intent().id
    );
    assert_eq!(
        old_response.admitted.as_ref().unwrap().intent().id,
        current.admitted.as_ref().unwrap().intent().id
    );
    assert!(old_error.generation < old_response.generation);
    assert!(old_response.generation < current.generation);
    let handle_id = rt.llm_task_handle.as_ref().unwrap().id();
    let current_generation = current.generation;
    let before = canonical_counts(&db).await;
    let (watch_tx, mut watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    old_error.outcome = LlmOutcome::NetworkError {
        message: "late actual task failure".into(),
    };
    rt.process_llm_completion(old_error).await.unwrap();
    assert_eq!(rt.llm_task_handle.as_ref().unwrap().id(), handle_id);
    assert!(rt.active_llm_attempt.is_some());
    assert_eq!(canonical_counts(&db).await, before);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());

    rt.process_llm_completion(current).await.unwrap();
    assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
    assert!(rt.pending_goal_request_checkpoint.is_some());
    let tool_handle = rt.tool_task_handle.as_ref().unwrap().id();
    let source_before_stale = rt
        .pending_goal_request_checkpoint
        .as_ref()
        .unwrap()
        .source
        .clone();
    let stamp_before_stale = rt
        .pending_goal_request_checkpoint
        .as_ref()
        .unwrap()
        .source_updated_at;
    watch_rx.borrow_and_update();
    while publications.try_recv().is_ok() {}
    rt.process_llm_completion(old_response).await.unwrap();
    assert_eq!(rt.tool_task_handle.as_ref().unwrap().id(), tool_handle);
    let pending = rt.pending_goal_request_checkpoint.as_ref().unwrap();
    assert_eq!(rt.llm_request_generation, current_generation);
    assert_eq!(pending.source, source_before_stale);
    assert_eq!(pending.source_updated_at, stamp_before_stale);
    assert_eq!(canonical_counts(&db).await, before);
    assert!(
        !watch_rx.has_changed().unwrap(),
        "stale envelope must not publish a different state"
    );
    assert!(
        publications.try_recv().is_err(),
        "stale envelope has no publication"
    );
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_pending_actual_round_refuses_next_dispatch_without_clearing_receipt() {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::tool_use(
            "checkpoint-first",
            "think",
            serde_json::json!({"thoughts":"no next dispatch yet"}),
        )]));
    let current = dispatch_response(&mut rt).await;
    let id = current.admitted.as_ref().unwrap().intent().id.clone();
    rt.process_llm_completion(current).await.unwrap();
    let before = canonical_counts(&db).await;
    assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
    assert_eq!(canonical_counts(&db).await, before);
    assert_eq!(
        rt.pending_goal_request_checkpoint
            .as_ref()
            .unwrap()
            .admitted
            .intent()
            .id,
        id
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_actual_summary_response_fails_closed_without_ordinary_continuation() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    let request = summary_state(&db, &mut rt, "unsupported-summary-checkpoint", 1).await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "actual summary",
        )]));
    send_summary(&mut rt, request).await;
    let summary = tokio::time::timeout(std::time::Duration::from_secs(30), rt.event_rx.recv())
        .await
        .expect("actual summary event liveness")
        .unwrap();
    assert!(
        matches!(&summary, Event::ContinuationResponse { summary, .. } if summary == "actual summary")
    );
    let before = canonical_counts(&db).await;
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(source.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    assert!(rt.process_event(summary).await.is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (stored.state, stored.state_updated_at),
        (source, source_stamp)
    );
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
}

#[tokio::test]
async fn conversation_goal_actual_response_rollback_publishes_neither_message_nor_selected_state() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "rollback this answer",
        )]));
    let completion = dispatch_response(&mut rt).await;
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(source.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    let before = canonical_counts(&db).await;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_canonical_response_state BEFORE UPDATE OF state ON conversations
        BEGIN SELECT RAISE(ABORT,'injected actual response state failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(rt.process_llm_completion(completion).await.is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (source.clone(), source_stamp)
    );
    assert_eq!(*watch_rx.borrow(), source);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}
