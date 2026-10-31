use super::*;
use crate::runtime::{deny_gate::CheckedToolCall, ToolExecutor, ToolRegistryExecutor};
use phoenix_core::domain::{
    db_schema::{InputOrigin, MessageContent},
    llm_types::ContentBlock,
    sm_event::{
        DirectTurnTurnId, PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
        SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
    },
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
};

struct Fixture {
    db: crate::db::Database,
    runtime: Arc<RuntimeManager>,
    caller: String,
    owner: ProductConversationId,
    target_transcript: String,
    turn_id: DirectTurnTurnId,
    executor: ToolRegistryExecutor,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_db(crate::db::Database::open_in_memory().await.unwrap()).await
    }

    async fn with_db(db: crate::db::Database) -> Self {
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let caller = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap()
            .id;
        let target = db
            .create_conversation(
                "goal-native-target",
                "goal-native-target",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let owner = target.product_conversation_id;
        db.set_auto_continue_on_context_exhaustion(
            &owner,
            phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .unwrap();
        let accepted = db
            .workflow_repository()
            .accept_authoritative_turn(&runtime_input(&caller, "native-goal-source"))
            .await
            .unwrap();
        let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("fresh runtime turn");
        };
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "Goal native test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let registry = runtime
            .coordinator_tool_registry(&caller, None)
            .await
            .unwrap();
        let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
        Self {
            db,
            runtime,
            caller,
            owner,
            target_transcript: target.id,
            turn_id: DirectTurnTurnId(turn_id.0),
            executor,
        }
    }

    fn ctx(&self) -> ToolContext {
        super::super::tests::context(&self.caller).with_direct_turn_id(self.turn_id)
    }

    async fn invoke(&self, name: &str, input: Value) -> ToolOutput {
        let call_id = uuid::Uuid::new_v4().to_string();
        let message_id = uuid::Uuid::new_v4().to_string();
        self.db
            .add_message(
                &message_id,
                &self.caller,
                &MessageContent::Agent(vec![ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: name.into(),
                    input: input.clone(),
                }]),
                None,
                None,
            )
            .await
            .unwrap();
        self.executor
            .execute(
                CheckedToolCall::cleared_for_test(name, input),
                self.ctx()
                    .with_tool_use_id(&call_id)
                    .with_svg_assistant_message_id(&message_id),
            )
            .await
            .expect("registered native Goal tool")
    }

    async fn prepare(&self, action: Value) -> Value {
        value(
            &self
                .invoke(
                    "prepare_goal",
                    json!({"target":self.target(),"action":action}),
                )
                .await,
        )
    }

    async fn apply(&self, prepared: &Value) -> ToolOutput {
        self.invoke(
            "apply_goal",
            json!({"action_reference":prepared["action_reference"]}),
        )
        .await
    }

    async fn work_counts(&self) -> (i64, i64, i64) {
        let workflows = sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        let turns = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        let queue = sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages")
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        (workflows, turns, queue)
    }

    fn target(&self) -> String {
        format!("@conv:{}", self.owner.as_str())
    }
}

fn value(output: &ToolOutput) -> Value {
    assert!(output.is_success(), "{}", output.output());
    serde_json::from_str(output.output()).unwrap()
}

#[tokio::test]
async fn native_goal_discovery_prepare_apply_read_and_replay_without_dispatch() {
    let f = Fixture::new().await;
    let definitions = f.executor.definitions().await;
    let migrations: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,name FROM _migrations WHERE version>=143 ORDER BY version")
            .fetch_all(f.db.pool())
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
            (149, "product_creation_image_retirement".into())
        ]
    );
    for name in [
        "read_goal",
        "prepare_goal",
        "apply_goal",
        "create_conversation",
        "rename_conversation",
    ] {
        assert_eq!(
            definitions.iter().filter(|tool| tool.name == name).count(),
            1,
            "{name}"
        );
    }
    for name in ["prepare_goal", "apply_goal"] {
        let description = &definitions
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .description;
        assert!(description.contains("persisted autoContinue preference"));
        assert!(description.contains("explicitly enabled"));
        assert!(description.contains("does not enable it"));
    }
    assert!(f
        .runtime
        .coordinator_tool_registry("goal-native-target", None)
        .await
        .is_err());
    let prepared = value(&f.invoke("prepare_goal", json!({"target":f.target(),"action":{"operation":"create","objective":"Native goal journey"}})).await);
    assert_eq!(prepared["expected_version"], 0);
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
    let action = json!({"action_reference":prepared["action_reference"]});
    let accepted = value(&f.invoke("apply_goal", action.clone()).await);
    assert_eq!(accepted["receipt"]["result"]["outcome"], "accepted");
    let replayed = value(&f.invoke("apply_goal", action).await);
    assert_eq!(accepted, replayed);
    let read = value(&f.invoke("read_goal", json!({"target":f.target()})).await);
    assert_eq!(read["history"].as_array().unwrap().len(), 1);
    assert_eq!(read["execution_state_included"], false);
    let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    let queue: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!((workflows, turns, queue), (1, 1, 0));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_goal_creation_and_replacement_require_auto_continue_at_apply() {
    use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion as Preference;

    let f = Fixture::new().await;
    let db = &f.db;
    let owner = &f.owner;
    let pref = |enabled| async move {
        db.set_auto_continue_on_context_exhaustion(
            owner,
            if enabled {
                Preference::Enabled
            } else {
                Preference::Disabled
            },
        )
        .await
        .unwrap();
    };
    let target_messages = || async {
        serde_json::to_value(f.db.get_messages(&f.target_transcript).await.unwrap()).unwrap()
    };
    let before_messages = target_messages().await;
    pref(false).await;

    let first = f
        .prepare(json!({"operation":"create","objective":"first approved objective"}))
        .await;
    let refused_create = f.apply(&first).await;
    assert!(!refused_create.is_success());
    assert!(
        refused_create
            .output()
            .contains("enable automatic continuation"),
        "{}",
        refused_create.output()
    );
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
    assert!(f
        .db
        .conversation_goal_history(
            &f.owner,
            phoenix_core::domain::conversation_goal::GoalVersion::INITIAL
        )
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(target_messages().await, before_messages);
    assert!(!f
        .db
        .auto_continue_on_context_exhaustion(&f.owner)
        .await
        .unwrap()
        .is_enabled());

    pref(true).await;
    let enabled_create = f
        .prepare(json!({"operation":"create","objective":"first approved objective"}))
        .await;
    assert!(f.apply(&enabled_create).await.is_success());
    pref(false).await;
    let accepted_replay = f.apply(&enabled_create).await;
    assert!(
        accepted_replay.is_success(),
        "accepted Coordinator replay is historical while disabled"
    );

    let replacement = f
        .prepare(json!({"operation":"replace","objective":"replacement while disabled"}))
        .await;
    let refused_replace = f.apply(&replacement).await;
    assert!(!refused_replace.is_success());
    assert!(refused_replace
        .output()
        .contains("enable automatic continuation"));
    assert_eq!(
        f.db.conversation_goal(&f.owner)
            .await
            .unwrap()
            .unwrap()
            .objective()
            .as_str(),
        "first approved objective"
    );
    assert_eq!(
        f.db.conversation_goal_history(
            &f.owner,
            phoenix_core::domain::conversation_goal::GoalVersion::INITIAL
        )
        .await
        .unwrap()
        .len(),
        1
    );
    assert_eq!(target_messages().await, before_messages);
    assert!(!f
        .db
        .auto_continue_on_context_exhaustion(&f.owner)
        .await
        .unwrap()
        .is_enabled());

    pref(true).await;
    let new_replacement = f
        .prepare(json!({"operation":"replace","objective":"replacement after explicit enable"}))
        .await;
    assert!(f.apply(&new_replacement).await.is_success());
}

#[tokio::test]
async fn native_goal_tools_refuse_raw_ids_authority_override_and_missing_turn() {
    let f = Fixture::new().await;
    for input in [
        json!({"target":f.owner.as_str()}),
        json!({"target":f.target(),"caller":"coordinator"}),
        json!({"target":f.target(),"after_version":-1}),
    ] {
        assert!(!f.invoke("read_goal", input).await.is_success());
    }
    assert!(!f
        .invoke("apply_goal", json!({"action_reference":"invented"}))
        .await
        .is_success());
    assert!(!f
        .invoke(
            "apply_goal",
            json!({"action_reference":uuid::Uuid::new_v4(),"target":f.target()})
        )
        .await
        .is_success());
    let prepare = tools(f.runtime.clone())
        .into_iter()
        .find(|t| t.name() == "prepare_goal")
        .unwrap();
    let missing = prepare
        .run(
            json!({"target":f.target(),"action":{"operation":"create","objective":"refused"}}),
            super::super::tests::context(&f.caller),
        )
        .await;
    assert!(!missing.is_success());
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
    let message_id = "missing-durable-turn-source";
    f.db.add_message(
        message_id,
        &f.caller,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "missing-turn-call".into(),
            name: "prepare_goal".into(),
            input: json!({}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    let missing_turn = prepare
        .run(
            json!({"target":f.target(),"action":{"operation":"create","objective":"no turn"}}),
            super::super::tests::context(&f.caller)
                .with_tool_use_id("missing-turn-call")
                .with_svg_assistant_message_id(message_id),
        )
        .await;
    assert!(!missing_turn.is_success());
    assert!(missing_turn.output().contains("durable turn provenance"));
}

#[tokio::test]
async fn native_goal_tools_recheck_coordinator_role_on_every_operation() {
    let f = Fixture::new().await;
    let ordinary = "goal-native-target";
    for name in ["read_goal", "prepare_goal", "apply_goal"] {
        let tool = tools(f.runtime.clone())
            .into_iter()
            .find(|t| t.name() == name)
            .unwrap();
        let input = match name {
            "read_goal" => json!({"target":f.target()}),
            "prepare_goal" => {
                json!({"target":f.target(),"action":{"operation":"create","objective":"refused"}})
            }
            _ => json!({"action_reference":uuid::Uuid::new_v4()}),
        };
        let output = tool
            .run(
                input,
                super::super::tests::context(ordinary).with_direct_turn_id(f.turn_id),
            )
            .await;
        assert!(!output.is_success(), "{name} accepted an ordinary caller");
    }
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
}
#[tokio::test]
async fn native_goal_explicit_replacement_and_dispositions_preserve_history_without_dispatch() {
    use phoenix_core::domain::conversation_goal::{GoalActor, GoalState};
    let f = Fixture::new().await;
    let before = f.work_counts().await;
    assert!(!f
        .invoke(
            "prepare_goal",
            json!({"target":f.target(),"action":{"operation":"replace","objective":"no Goal yet"}})
        )
        .await
        .is_success());
    let create = f
        .prepare(json!({"operation":"create","objective":"first approved objective"}))
        .await;
    value(&f.apply(&create).await);
    let first = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    assert!(!f.invoke("prepare_goal", json!({"target":f.target(),"action":{"operation":"create","objective":"blind overwrite"}})).await.is_success());
    let replace = f
        .prepare(json!({"operation":"replace","objective":"explicit replacement"}))
        .await;
    assert_eq!(replace["observed_goal_id"], first.goal_id().as_str());
    assert_eq!(replace["expected_version"], first.version().get());
    assert_eq!(
        f.db.conversation_goal(&f.owner).await.unwrap().unwrap(),
        first
    );
    value(&f.apply(&replace).await);
    let replacement = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    assert_ne!(replacement.goal_id(), first.goal_id());
    assert_eq!(replacement.objective().as_str(), "explicit replacement");
    for disposition in [
        json!({"state":"blocked","reason":"needs user decision"}),
        json!({"state":"underway"}),
        json!({"state":"complete","reason":"approved result delivered"}),
    ] {
        let prepared = f
            .prepare(json!({"operation":"state","disposition":disposition}))
            .await;
        value(&f.apply(&prepared).await);
        assert_eq!(
            f.db.conversation_goal(&f.owner)
                .await
                .unwrap()
                .unwrap()
                .goal_id(),
            replacement.goal_id()
        );
    }
    assert!(matches!(
        f.db.conversation_goal(&f.owner)
            .await
            .unwrap()
            .unwrap()
            .state(),
        GoalState::Complete(_)
    ));
    let history =
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap();
    assert_eq!(history.len(), 5);
    for (index, entry) in history.iter().enumerate() {
        assert_eq!(
            entry.snapshot().version().get(),
            i64::try_from(index).unwrap() + 1
        );
        assert!(
            matches!(&entry.mutation().attribution.requested_by, GoalActor::Coordinator(source) if source.transcript_id.as_str() == f.caller && source.execution.turn_id() == i64::try_from(f.turn_id.0).unwrap())
        );
    }
    assert_eq!(f.work_counts().await, before);
}

#[tokio::test]
async fn native_goal_conflict_and_busy_receipts_replay_without_new_mutations() {
    let f = Fixture::new().await;
    let before = f.work_counts().await;
    let stale = f
        .prepare(json!({"operation":"create","objective":"stale observation"}))
        .await;
    let winner = f
        .prepare(json!({"operation":"create","objective":"newer approval"}))
        .await;
    value(&f.apply(&winner).await);
    let refused = f.apply(&stale).await;
    assert!(!refused.is_success());
    let receipt: Value = serde_json::from_str(refused.output()).unwrap();
    assert_eq!(receipt["receipt"]["result"]["reason"], "version");
    let retry = f.apply(&stale).await;
    assert!(!retry.is_success());
    assert_eq!(retry.output(), refused.output());
    assert_eq!(
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    let blocked = f.prepare(json!({"operation":"state","disposition":{"state":"blocked","reason":"retained busy test"}})).await;
    let gate = f
        .runtime
        .try_lock_product_message_admission(f.owner.as_str())
        .await
        .expect("isolated product gate");
    let busy = f.apply(&blocked).await;
    assert!(!busy.is_success());
    drop(gate);
    let retry = f.apply(&blocked).await;
    assert!(!retry.is_success());
    assert_eq!(retry.output(), busy.output());
    let receipt: Value = serde_json::from_str(busy.output()).unwrap();
    assert_eq!(receipt["receipt"]["result"]["reason"], "busy");
    assert_eq!(
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.work_counts().await, before);
}

#[tokio::test]
async fn native_goal_target_rollover_follows_live_owner_and_replays_old_reference() {
    let f = Fixture::new().await;
    let prepared = f
        .prepare(json!({"operation":"create","objective":"stable owner through rollover"}))
        .await;
    f.db.update_conversation_state(
        "goal-native-target",
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "isolated target rollover".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) =
        f.db.continue_conversation("goal-native-target")
            .await
            .unwrap()
    else {
        panic!("isolated continuation");
    };
    assert_eq!(successor.product_conversation_id, f.owner);
    let before = f.work_counts().await;
    let accepted = f.apply(&prepared).await;
    value(&accepted);
    let retry = f.apply(&prepared).await;
    assert!(retry.is_success());
    assert_eq!(retry.output(), accepted.output());
    let read = value(&f.invoke("read_goal", json!({"target":f.target()})).await);
    assert_eq!(read["history"].as_array().unwrap().len(), 1);
    assert_eq!(f.work_counts().await, before);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_suspended_goal_replacement_uses_auto_continue_admission_and_preserves_history() {
    use phoenix_core::domain::conversation_goal::{GoalDisposition, GoalState, GoalText};
    use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion as Preference;
    use phoenix_db::conversation_goals::user_commands::{GoalUserChange, GoalUserCommand};

    let f = Fixture::new().await;
    let prepared = f
        .prepare(json!({"operation":"create","objective":"suspension boundary"}))
        .await;
    value(&f.apply(&prepared).await);
    let current = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    let suspend = GoalUserCommand {
        owner: f.owner.clone(),
        principal_id: uuid::Uuid::new_v4(),
        request_id: uuid::Uuid::new_v4(),
        expected_version: current.version(),
        change: GoalUserChange::State {
            goal_id: current.goal_id().clone(),
            disposition: GoalDisposition::Suspended(GoalText::parse("user stopped work").unwrap()),
        },
    };
    f.db.apply_user_goal_command(
        &suspend,
        "goal-native-target",
        || {
            Ok(
                phoenix_core::domain::conversation_goal::GoalTimestampUs::parse(
                    chrono::Utc::now().timestamp_micros() + 1_000_000,
                )?,
            )
        },
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let work_before = f.work_counts().await;
    let suspended = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    let old_goal_id = suspended.goal_id().clone();
    assert!(matches!(suspended.state(), GoalState::Suspended(_)));

    let underway = f
        .invoke(
            "prepare_goal",
            json!({
                "target": f.target(),
                "action": {"operation":"state","disposition":{"state":"underway"}}
            }),
        )
        .await;
    assert!(!underway.is_success());
    assert!(underway.output().contains("explicit Start"));
    assert!(!f
        .invoke("prepare_goal", json!({"target":f.target(),"action":{"operation":"state","disposition":{"state":"suspended","reason":"not a Stop tool"}}}))
        .await
        .is_success());
    assert_eq!(
        f.db.conversation_goal(&f.owner).await.unwrap(),
        Some(suspended.clone())
    );

    f.db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Disabled)
        .await
        .unwrap();
    let history_before =
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap();
    let disabled = f
        .prepare(json!({"operation":"replace","objective":"replacement"}))
        .await;
    let refused = f.apply(&disabled).await;
    assert!(!refused.is_success());
    assert!(refused.output().contains("enable automatic continuation"));
    assert_eq!(
        f.db.conversation_goal(&f.owner).await.unwrap(),
        Some(suspended.clone())
    );
    assert_eq!(
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap(),
        history_before
    );

    f.db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Enabled)
        .await
        .unwrap();
    let enabled = f
        .prepare(json!({"operation":"replace","objective":"replacement"}))
        .await;
    let accepted = f.apply(&enabled).await;
    assert!(accepted.is_success(), "{}", accepted.output());
    let replacement = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    assert_ne!(replacement.goal_id(), &old_goal_id);
    assert_eq!(replacement.objective().as_str(), "replacement");
    let history_after =
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap();
    assert_eq!(history_after.len(), history_before.len() + 1);
    assert_eq!(
        &history_after[..history_before.len()],
        history_before.as_slice()
    );
    assert!(history_after.iter().any(|entry| {
        entry.snapshot().goal_id() == &old_goal_id
            && matches!(entry.snapshot().state(), GoalState::Suspended(_))
    }));

    f.db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Disabled)
        .await
        .unwrap();
    let replay = f.apply(&enabled).await;
    assert!(replay.is_success());
    assert_eq!(replay.output(), accepted.output());
    assert_eq!(
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap(),
        history_after
    );
    assert_eq!(f.work_counts().await, work_before);
}

#[tokio::test]
async fn native_goal_prepared_reference_survives_coordinator_continuation() {
    let f = Fixture::new().await;
    let prepared = f
        .prepare(json!({"operation":"create","objective":"retained Coordinator authority"}))
        .await;
    f.db.update_conversation_state(
        &f.caller,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "isolated Coordinator rollover".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) =
        f.db.continue_conversation(&f.caller).await.unwrap()
    else {
        panic!("isolated Coordinator continuation");
    };
    let registry = f
        .runtime
        .coordinator_tool_registry(&successor.id, None)
        .await
        .unwrap();
    assert!(f
        .runtime
        .coordinator_tool_registry(&f.caller, None)
        .await
        .is_err());
    for (name, input) in [
        ("read_goal", json!({"target":f.target()})),
        (
            "prepare_goal",
            json!({"target":f.target(),"action":{"operation":"create","objective":"stale caller"}}),
        ),
        (
            "apply_goal",
            json!({"action_reference":prepared["action_reference"]}),
        ),
    ] {
        assert!(
            !f.invoke(name, input).await.is_success(),
            "retained old registry accepted {name}"
        );
    }
    let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let before = f.work_counts().await;
    let apply = || {
        CheckedToolCall::cleared_for_test(
            "apply_goal",
            json!({"action_reference":prepared["action_reference"]}),
        )
    };
    let ctx = || super::super::tests::context(&successor.id);
    let accepted = executor.execute(apply(), ctx()).await.unwrap();
    value(&accepted);
    let replay = executor.execute(apply(), ctx()).await.unwrap();
    assert!(replay.is_success());
    assert_eq!(replay.output(), accepted.output());
    assert_eq!(f.work_counts().await, before);
    let history =
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap();
    assert_eq!(history.len(), 1);
    assert!(
        matches!(&history[0].mutation().attribution.requested_by, phoenix_core::domain::conversation_goal::GoalActor::Coordinator(source) if source.transcript_id.as_str()==f.caller)
    );
}

#[tokio::test]
async fn native_goal_receipt_replays_after_database_reopen_and_service_reconstruction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("isolated-native-goal.sqlite");
    let f = Fixture::with_db(
        crate::db::Database::open(path.to_str().unwrap())
            .await
            .unwrap(),
    )
    .await;
    let prepared = f
        .prepare(json!({"operation":"create","objective":"retained after reopen"}))
        .await;
    let accepted = f.apply(&prepared).await;
    value(&accepted);
    let caller = f.caller.clone();
    let owner = f.owner.clone();
    let before = f.work_counts().await;
    f.db.pool().close().await;
    drop(f);
    let db = crate::db::Database::open(path.to_str().unwrap())
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        Arc::new(phoenix_llm::ModelRegistry::new_empty()),
        crate::platform::PlatformCapability::None {
            details: "isolated Goal reopen".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    let registry = runtime
        .coordinator_tool_registry(&caller, None)
        .await
        .unwrap();
    let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let replay = executor
        .execute(
            CheckedToolCall::cleared_for_test(
                "apply_goal",
                json!({"action_reference":prepared["action_reference"]}),
            ),
            super::super::tests::context(&caller),
        )
        .await
        .unwrap();
    assert!(replay.is_success());
    assert_eq!(replay.output(), accepted.output());
    assert_eq!(
        db.conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let queue: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!((workflows, turns, queue), before);
}

#[tokio::test]
async fn native_goal_accepted_and_refused_receipts_replay_through_coordinator_successor() {
    let f = Fixture::new().await;
    let accepted_ref = f
        .prepare(json!({"operation":"create","objective":"accepted before continuation"}))
        .await;
    let accepted = f.apply(&accepted_ref).await;
    value(&accepted);
    let busy_ref = f.prepare(json!({"operation":"state","disposition":{"state":"blocked","reason":"refused before continuation"}})).await;
    let gate = f
        .runtime
        .try_lock_product_message_admission(f.owner.as_str())
        .await
        .unwrap();
    let refused = f.apply(&busy_ref).await;
    assert!(!refused.is_success());
    drop(gate);
    f.db.update_conversation_state(
        &f.caller,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "retained receipt continuation".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) =
        f.db.continue_conversation(&f.caller).await.unwrap()
    else {
        panic!("fresh continuation")
    };
    let registry = f
        .runtime
        .coordinator_tool_registry(&successor.id, None)
        .await
        .unwrap();
    let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let before = f.work_counts().await;
    for (reference, original) in [(&accepted_ref, &accepted), (&busy_ref, &refused)] {
        let replay = executor
            .execute(
                CheckedToolCall::cleared_for_test(
                    "apply_goal",
                    json!({"action_reference":reference["action_reference"]}),
                ),
                super::super::tests::context(&successor.id),
            )
            .await
            .unwrap();
        assert_eq!(replay.is_success(), original.is_success());
        assert_eq!(replay.output(), original.output());
    }
    assert_eq!(
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.work_counts().await, before);
}

#[tokio::test]
async fn native_goal_reopen_preserves_pending_preparation_and_refused_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending-refused.sqlite");
    let f = Fixture::with_db(
        crate::db::Database::open(path.to_str().unwrap())
            .await
            .unwrap(),
    )
    .await;
    let pending = f
        .prepare(json!({"operation":"create","objective":"pending before reopen"}))
        .await;
    let busy_ref = f
        .prepare(json!({"operation":"create","objective":"refused before reopen"}))
        .await;
    let gate = f
        .runtime
        .try_lock_product_message_admission(f.owner.as_str())
        .await
        .unwrap();
    let refused = f.apply(&busy_ref).await;
    assert!(!refused.is_success());
    drop(gate);
    let caller = f.caller.clone();
    let owner = f.owner.clone();
    let before = f.work_counts().await;
    f.db.pool().close().await;
    drop(f);
    let db = crate::db::Database::open(path.to_str().unwrap())
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        Arc::new(phoenix_llm::ModelRegistry::new_empty()),
        crate::platform::PlatformCapability::None {
            details: "pending reopen".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    let registry = runtime
        .coordinator_tool_registry(&caller, None)
        .await
        .unwrap();
    let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let replay = executor
        .execute(
            CheckedToolCall::cleared_for_test(
                "apply_goal",
                json!({"action_reference":busy_ref["action_reference"]}),
            ),
            super::super::tests::context(&caller),
        )
        .await
        .unwrap();
    assert!(!replay.is_success());
    assert_eq!(replay.output(), refused.output());
    let applied = executor
        .execute(
            CheckedToolCall::cleared_for_test(
                "apply_goal",
                json!({"action_reference":pending["action_reference"]}),
            ),
            super::super::tests::context(&caller),
        )
        .await
        .unwrap();
    value(&applied);
    assert_eq!(
        db.conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    let counts:(i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM workflows),(SELECT COUNT(*) FROM durable_turns),(SELECT COUNT(*) FROM steering_messages)").fetch_one(db.pool()).await.unwrap();
    assert_eq!(counts, before);
}

fn runtime_input(transcript: &str, key: &str) -> phoenix_db::workflow::AcceptAuthoritativeTurn {
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
    phoenix_db::workflow::AcceptAuthoritativeTurn {
        client_key: ClientTurnKey::new(key).unwrap(),
        prepared: PreparedTurn::from_exact_payload(
            &ConversationAuthority(transcript.into()),
            payload.to_exact_bytes().unwrap(),
        ),
        disposition: AcceptedDisposition::Runtime,
        accepted_at: Timestamp(1),
    }
}
