use super::*;
use crate::runtime::{deny_gate::CheckedToolCall, ToolExecutor, ToolRegistryExecutor};
use phoenix_core::domain::{
    db_schema::{InputOrigin, MessageContent},
    instance_identity::{PeerBearerCredential, PeerTlsTrust},
    llm_types::ContentBlock,
    sm_event::{
        DirectTurnTurnId, PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
        SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
    },
    sm_state::{AssistantMessage, ToolCall, ToolInput},
};
use phoenix_db::ConvState;
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
            .established()
            .expect("native Goal fixture turn admission must have an established durable fact")
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

    async fn source_context(&self, name: &str, input: Value) -> ToolContext {
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
        self.ctx()
            .with_tool_use_id(&call_id)
            .with_svg_assistant_message_id(&message_id)
    }

    async fn invoke(&self, name: &str, input: Value) -> ToolOutput {
        let context = self.source_context(name, input.clone()).await;
        self.executor
            .execute(CheckedToolCall::cleared_for_test(name, input), context)
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

async fn assert_remote_source_race_refused(operation: Operation, terminalize: bool) {
    use phoenix_core::domain::instance_identity::{InstanceId, PeerBearerCredential, PeerTlsTrust};

    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let requests = tokio::spawn(async move {
        let mut dispatched = 0;
        loop {
            tokio::select! {
                _ = &mut stopped => return dispatched,
                connection = listener.accept() => {
                    let (stream, _) = connection.unwrap();
                    dispatched += 1;
                    drop(stream);
                }
            }
        }
    });
    f.runtime
        .db()
        .save_federation_peer_connection(
            InstanceId::new(),
            &"race-peer".parse().unwrap(),
            &format!("https://{address}").parse().unwrap(),
            &PeerBearerCredential::parse(format!("phx_peer_{}", "b".repeat(43))).unwrap(),
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
    let (name, mut input) = match operation {
        Operation::Prepare => (
            "prepare_goal",
            json!({
                "target":format!("@conv:{}", f.owner.as_str()),
                "action":{"operation":"create","objective":"Must not dispatch"}
            }),
        ),
        Operation::Apply => ("apply_goal", json!({"action_reference":Uuid::new_v4()})),
    };
    input["instance"] = json!("race-peer");
    let context = f.source_context(name, input.clone()).await;
    let turn = context.direct_turn_id().unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let service = GoalCommandService::new(Arc::clone(&f.runtime))
        .with_remote_source_identity_barrier(Arc::clone(&barrier));
    let tool = crate::coordinator_tools::instance::RouteTool::boxed(GoalTool {
        service,
        runtime: Arc::clone(&f.runtime),
        operation,
    });
    let task = tokio::spawn(async move { tool.run(input, context).await });
    barrier.wait().await;
    let product = f
        .runtime
        .db()
        .get_conversation(&f.caller)
        .await
        .unwrap()
        .product_conversation_id;
    let admission = f
        .runtime
        .try_lock_product_message_admission(product.as_str())
        .await
        .unwrap();
    if terminalize {
        sqlx::query("UPDATE durable_turns SET terminal_kind='Cancelled',owns_conversation=0 WHERE turn_id=?")
            .bind(i64::try_from(turn.0).unwrap()).execute(f.runtime.db().pool()).await.unwrap();
    } else {
        sqlx::query("UPDATE workflows SET generation=generation+1 WHERE workflow_id=(SELECT workflow_id FROM durable_turns WHERE turn_id=?)")
            .bind(i64::try_from(turn.0).unwrap()).execute(f.runtime.db().pool()).await.unwrap();
    }
    drop(admission);
    barrier.wait().await;
    let output = task.await.unwrap();
    assert!(!output.is_success());
    assert!(
        output
            .output()
            .contains("durable Coordinator turn provenance unavailable"),
        "{}",
        output.output()
    );
    stop.send(()).unwrap();
    assert_eq!(
        requests.await.unwrap(),
        0,
        "stale source dispatched remotely"
    );
    assert!(f
        .runtime
        .db()
        .conversation_goal(&f.owner)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn remote_prepare_revalidates_terminalized_turn_after_identity_lookup() {
    assert_remote_source_race_refused(Operation::Prepare, true).await;
}

#[tokio::test]
async fn remote_apply_revalidates_terminalized_turn_after_identity_lookup() {
    assert_remote_source_race_refused(Operation::Apply, true).await;
}

#[tokio::test]
async fn remote_prepare_revalidates_stale_generation_after_identity_lookup() {
    assert_remote_source_race_refused(Operation::Prepare, false).await;
}

#[tokio::test]
async fn remote_apply_revalidates_stale_generation_after_identity_lookup() {
    assert_remote_source_race_refused(Operation::Apply, false).await;
}

#[tokio::test]
async fn remote_source_requires_exact_tool_call_and_retains_admission() {
    let f = Fixture::new().await;
    let context = f.source_context("prepare_goal", json!({})).await;
    let turn = context.direct_turn_id().unwrap();
    let call = context.source_tool_call().unwrap();
    let service = GoalCommandService::new(Arc::clone(&f.runtime));
    let (admission, source) = service
        .admit_remote_source(&f.caller, &call, turn, "prepare_goal")
        .await
        .unwrap();
    assert!(f
        .runtime
        .try_lock_product_message_admission(source.product_conversation_id.as_str())
        .await
        .is_none());
    drop(admission);
    let mut wrong_call = call.clone();
    wrong_call.tool_use_id = "not-the-server-owned-call".into();
    assert!(service
        .admit_remote_source(&f.caller, &wrong_call, turn, "prepare_goal")
        .await
        .is_err());
    assert!(service
        .admit_remote_source(&f.caller, &call, turn, "apply_goal")
        .await
        .is_err());
    assert!(f
        .runtime
        .try_lock_product_message_admission(source.product_conversation_id.as_str())
        .await
        .is_some());
}

#[tokio::test]
async fn explicit_local_goal_read_preserves_snapshot_wait_and_authority() {
    let f = Fixture::new().await;
    let prepared = f
        .prepare(json!({"operation":"create","objective":"Local selector parity"}))
        .await;
    let applied = value(&f.apply(&prepared).await);
    assert_eq!(applied["receipt"]["result"]["outcome"], "accepted");
    let before = f.work_counts().await;
    for mut input in [
        json!({"target":f.target(),"after_version":0}),
        json!({"target":f.target(),"after_version":1}),
        json!({"target":f.target(),"wait":{"minimum_version":1,"timeout_seconds":1}}),
    ] {
        let omitted = value(&f.invoke("read_goal", input.clone()).await);
        input["instance"] = json!("local");
        let explicit = value(&f.invoke("read_goal", input.clone()).await);
        assert_eq!(omitted, explicit);
        input["instance"] = Value::Null;
        assert_eq!(omitted, value(&f.invoke("read_goal", input.clone()).await));
        input["instance"] = json!("local");
        let unauthorized = f
            .executor
            .execute(
                CheckedToolCall::cleared_for_test("read_goal", input),
                super::super::tests::context(&f.target_transcript),
            )
            .await
            .expect("registered read_goal tool");
        assert!(!unauthorized.is_success());
        assert!(unauthorized.output().contains("Coordinator"));
    }
    for selector in [
        json!("missing-peer"),
        json!("Local"),
        json!(" local "),
        json!(""),
        json!(false),
        json!({}),
    ] {
        assert!(!f
            .invoke(
                "read_goal",
                json!({"target":f.target(), "instance":selector})
            )
            .await
            .is_success());
    }
    assert_eq!(before, f.work_counts().await);
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
    for name in [
        "read_goal",
        "prepare_goal",
        "apply_goal",
        "create_conversation",
        "rename_conversation",
        "continue_conversation",
        "read_automatic_continuation",
        "set_automatic_continuation",
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
    let prepare = tools(&f.runtime)
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
        let tool = tools(&f.runtime)
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
#[allow(clippy::too_many_lines)]
async fn native_goal_prepare_after_owner_and_coordinator_continuation_uses_in_flight_source() {
    use phoenix_core::domain::conversation_goal::GoalActor;

    let f = Fixture::new().await;
    let initial_preparation = f
        .prepare(json!({"operation":"create","objective":"initial Goal"}))
        .await;
    value(&f.apply(&initial_preparation).await);
    let initial_goal = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();

    f.db.update_conversation_state(
        &f.target_transcript,
        &ConvState::ContextExhausted {
            summary: "ordinary owner continuation".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(owner_successor) =
        f.db.continue_conversation(&f.target_transcript)
            .await
            .unwrap()
    else {
        panic!("fresh ordinary continuation")
    };
    assert_eq!(owner_successor.product_conversation_id, f.owner);

    f.db.update_conversation_state(
        &f.caller,
        &ConvState::ContextExhausted {
            summary: "Coordinator continuation".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(coordinator_successor) =
        f.db.continue_conversation(&f.caller).await.unwrap()
    else {
        panic!("fresh Coordinator continuation")
    };
    assert!(f
        .runtime
        .coordinator_tool_registry(&f.caller, None)
        .await
        .is_err());

    let accepted =
        f.db.workflow_repository()
            .accept_authoritative_turn(&runtime_input(
                &coordinator_successor.id,
                "continued-goal-replacement",
            ))
            .await
            .established()
            .expect("continued Coordinator turn admission must establish a durable fact")
            .unwrap();
    let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("fresh continued Coordinator Runtime turn")
    };
    let turn_id = DirectTurnTurnId(turn_id.0);

    let prepare_message_id = uuid::Uuid::new_v4().to_string();
    let prepare_tool_use_id = uuid::Uuid::new_v4().to_string();
    let prepare_input = json!({
        "target": f.target(),
        "action": {"operation":"replace","objective":"continued replacement"}
    });
    f.db.update_conversation_state(
        &coordinator_successor.id,
        &ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &prepare_tool_use_id,
                ToolInput::Unknown {
                    name: "prepare_goal".into(),
                    input: prepare_input.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                prepare_message_id.clone(),
                vec![ContentBlock::ToolUse {
                    id: prepare_tool_use_id.clone(),
                    name: "prepare_goal".into(),
                    input: prepare_input.clone(),
                }],
                None,
                None,
            ),
        },
    )
    .await
    .unwrap();
    let persisted_prepare_message: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE conversation_id=? AND message_id=?",
    )
    .bind(&coordinator_successor.id)
    .bind(&prepare_message_id)
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(persisted_prepare_message, 0);

    let registry = f
        .runtime
        .coordinator_tool_registry(&coordinator_successor.id, None)
        .await
        .unwrap();
    let executor = ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let prepared = executor
        .execute(
            CheckedToolCall::cleared_for_test("prepare_goal", prepare_input.clone()),
            super::super::tests::context(&coordinator_successor.id)
                .with_direct_turn_id(turn_id)
                .with_tool_use_id(&prepare_tool_use_id)
                .with_svg_assistant_message_id(&prepare_message_id),
        )
        .await
        .unwrap();
    let prepared = value(&prepared);
    assert_eq!(prepared["target"], f.target());
    assert_eq!(prepared["expected_version"], initial_goal.version().get());
    assert!(uuid::Uuid::parse_str(prepared["action_reference"].as_str().unwrap()).is_ok());
    f.db.add_message(
        &prepare_message_id,
        &coordinator_successor.id,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: prepare_tool_use_id.clone(),
            name: "prepare_goal".into(),
            input: prepare_input,
        }]),
        None,
        None,
    )
    .await
    .unwrap();

    let before_apply = f.work_counts().await;
    let apply_tool_use_id = uuid::Uuid::new_v4().to_string();
    let apply_message_id = uuid::Uuid::new_v4().to_string();
    let apply_input = json!({"action_reference":prepared["action_reference"]});
    f.db.add_message(
        &apply_message_id,
        &coordinator_successor.id,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: apply_tool_use_id.clone(),
            name: "apply_goal".into(),
            input: apply_input.clone(),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    let apply = || CheckedToolCall::cleared_for_test("apply_goal", apply_input.clone());
    let apply_context = || {
        super::super::tests::context(&coordinator_successor.id)
            .with_direct_turn_id(turn_id)
            .with_tool_use_id(&apply_tool_use_id)
            .with_svg_assistant_message_id(&apply_message_id)
    };
    let accepted = executor.execute(apply(), apply_context()).await.unwrap();
    value(&accepted);
    let replay = executor.execute(apply(), apply_context()).await.unwrap();
    assert!(replay.is_success());
    assert_eq!(replay.output(), accepted.output());
    assert_eq!(f.work_counts().await, before_apply);

    let current_owner =
        f.db.get_ordinary_product_conversation(&f.owner)
            .await
            .unwrap();
    assert_eq!(current_owner.latest_transcript_row_id, owner_successor.id);
    let replacement = f.db.conversation_goal(&f.owner).await.unwrap().unwrap();
    assert_ne!(replacement.goal_id(), initial_goal.goal_id());
    assert_eq!(
        replacement.version().get(),
        initial_goal.version().get() + 1
    );
    assert_eq!(replacement.objective().as_str(), "continued replacement");
    let history =
        f.db.conversation_goal_history(&f.owner, GoalVersion::INITIAL)
            .await
            .unwrap();
    assert_eq!(history.len(), 2);
    assert!(matches!(
        &history[1].mutation().attribution.requested_by,
        GoalActor::Coordinator(source)
            if source.product_conversation_id == coordinator_successor.product_conversation_id
                && source.transcript_id.as_str() == coordinator_successor.id
                && source.message_id.as_str() == prepare_message_id
                && source.tool_use_id.as_str() == prepare_tool_use_id
                && source.execution.turn_id() == i64::try_from(turn_id.0).unwrap()
    ));
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

#[tokio::test]
async fn production_coordinator_registry_has_exact_instance_selector_contract() {
    let f = Fixture::new().await;
    let registry = f
        .runtime
        .coordinator_tool_registry(&f.caller, None)
        .await
        .unwrap();
    let definitions = registry.definitions();
    let selector_tools = [
        "read_conversation",
        "query_database",
        "resolve_reference",
        "send_conversation_message",
        "watch_conversation",
        "unwatch_conversation",
        "list_watched_conversations",
        "read_goal",
        "continue_conversation",
        "prepare_goal",
        "apply_goal",
        "create_conversation",
        "rename_conversation",
    ];

    assert!(!definitions
        .iter()
        .any(|definition| definition.name == "remote_query_database"));
    for name in selector_tools {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == name)
            .unwrap_or_else(|| panic!("missing production coordinator tool {name}"));
        let expected = if matches!(
            name,
            "send_conversation_message"
                | "watch_conversation"
                | "unwatch_conversation"
                | "list_watched_conversations"
                | "prepare_goal"
                | "apply_goal"
                | "create_conversation"
                | "rename_conversation"
        ) {
            crate::coordinator_tools::instance::InstanceSelector::schema()
        } else {
            crate::coordinator_tools::instance::InstanceSelector::nullable_schema()
        };
        assert_eq!(
            definition.input_schema["properties"]["instance"], expected,
            "{name} must expose its standard instance selector"
        );
    }
    for name in ["read_automatic_continuation", "set_automatic_continuation"] {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == name)
            .unwrap_or_else(|| panic!("missing production coordinator tool {name}"));
        assert_eq!(
            definition.input_schema["properties"]["instance"],
            json!({"type": "string", "minLength": 1}),
            "{name} retains its existing separate instance contract"
        );
    }
    for definition in definitions.iter().filter(|definition| {
        !selector_tools.contains(&definition.name.as_str())
            && !matches!(
                definition.name.as_str(),
                "read_automatic_continuation" | "set_automatic_continuation"
            )
    }) {
        assert!(
            definition.input_schema["properties"]
                .get("instance")
                .is_none(),
            "{} must not expose an instance selector",
            definition.name
        );
    }
}

#[tokio::test]
async fn goal_tools_omit_or_accept_exact_local_instance_and_reject_invalid_remote_without_fallback()
{
    let f = Fixture::new().await;
    let omitted = value(
        &f.invoke(
            "prepare_goal",
            json!({"target":f.target(),"action":{"operation":"create","objective":"route parity"}}),
        )
        .await,
    );
    let local = value(&f.invoke("prepare_goal", json!({"target":f.target(),"instance":"local","action":{"operation":"create","objective":"route parity"}})).await);
    assert_eq!(omitted["target"], local["target"]);
    assert_eq!(omitted["action"], local["action"]);
    assert_eq!(omitted["expected_version"], local["expected_version"]);
    assert!(local.get("observed_goal_id").is_some());
    assert!(local.get("instance_id").is_none());
    let prepare_description = f
        .runtime
        .coordinator_tool_registry(&f.caller, None)
        .await
        .unwrap()
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "prepare_goal")
        .unwrap()
        .description;
    assert!(prepare_description.contains("identifies the selected peer"));
    let before = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM conversation_goal_preparations WHERE owner_id=?",
    )
    .bind(f.owner.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    for selector in [
        json!("missing-peer"),
        json!("Local"),
        json!(" local "),
        json!(null),
        json!(false),
    ] {
        let output = f.invoke("prepare_goal", json!({"target":f.target(),"instance":selector,"action":{"operation":"create","objective":"no fallback"}})).await;
        assert!(!output.is_success());
    }
    let local_id = f.db.instance_id().await.unwrap();
    f.db.save_federation_peer_connection(
        local_id,
        &"self-alias".parse().unwrap(),
        &"https://self.invalid".parse().unwrap(),
        &PeerBearerCredential::parse(format!("phx_peer_{}", "s".repeat(43))).unwrap(),
        &PeerTlsTrust::PlatformRoots,
    )
    .await
    .unwrap();
    let work_before = f.work_counts().await;
    let receipts_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    for selector in [local_id.to_string(), "self-alias".to_owned()] {
        let prepare = f
            .invoke(
                "prepare_goal",
                json!({"target":f.target(),"instance":selector,"action":{"operation":"create","objective":"never routed to self"}}),
            )
            .await;
        assert!(!prepare.is_success(), "{}", prepare.output());
        let apply = f
            .invoke(
                "apply_goal",
                json!({"instance":selector,"action_reference":Uuid::new_v4()}),
            )
            .await;
        assert!(!apply.is_success(), "{}", apply.output());
    }
    let receipts_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(f.work_counts().await, work_before);
    assert_eq!(receipts_after, receipts_before);
    let after = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM conversation_goal_preparations WHERE owner_id=?",
    )
    .bind(f.owner.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn watch_tools_treat_omitted_and_exact_local_as_equal_and_reject_invalid_selectors_without_effects(
) {
    let f = Fixture::new().await;
    let target = f.owner.as_str();

    let omitted_watch = value(
        &f.invoke(
            "watch_conversation",
            json!({"product_conversation_id":target}),
        )
        .await,
    );
    let explicit_local_watch = value(
        &f.invoke(
            "watch_conversation",
            json!({"product_conversation_id":target,"instance":"local"}),
        )
        .await,
    );
    assert_eq!(omitted_watch, explicit_local_watch);

    let omitted_unwatch = value(
        &f.invoke(
            "unwatch_conversation",
            json!({"product_conversation_id":target}),
        )
        .await,
    );
    assert_eq!(omitted_unwatch["ended"], true);
    let _ = f
        .invoke(
            "watch_conversation",
            json!({"product_conversation_id":target}),
        )
        .await;
    let explicit_local_unwatch = value(
        &f.invoke(
            "unwatch_conversation",
            json!({"product_conversation_id":target,"instance":"local"}),
        )
        .await,
    );
    assert_eq!(omitted_unwatch, explicit_local_unwatch);

    let omitted_list = value(&f.invoke("list_watched_conversations", json!({})).await);
    let explicit_local_list = value(
        &f.invoke("list_watched_conversations", json!({"instance":"local"}))
            .await,
    );
    assert_eq!(omitted_list, explicit_local_list);

    let _ = f
        .invoke(
            "watch_conversation",
            json!({"product_conversation_id":target}),
        )
        .await;
    let before = f.db.list_coordinator_watches().await.unwrap();
    for (name, input) in [
        (
            "watch_conversation",
            json!({"product_conversation_id":target}),
        ),
        (
            "unwatch_conversation",
            json!({"product_conversation_id":target}),
        ),
        ("list_watched_conversations", json!({})),
    ] {
        for selector in [
            json!("missing-peer"),
            json!("Local"),
            json!(" local "),
            json!(false),
        ] {
            let mut input = input.clone();
            input["instance"] = selector;
            let output = f.invoke(name, input).await;
            assert!(
                !output.is_success(),
                "{name} unexpectedly accepted selector"
            );
            assert_eq!(
                serde_json::to_value(&before).unwrap(),
                serde_json::to_value(f.db.list_coordinator_watches().await.unwrap()).unwrap(),
                "{name} with invalid selector must not change local watches"
            );
        }
    }
}

#[tokio::test]
async fn malformed_remote_reads_reject_before_saved_peer_lookup() {
    let f = Fixture::new().await;
    for (name, input, expected) in [
        (
            "read_conversation",
            json!({"instance":"missing-peer", "conversation_id":"@conv:target", "cursor":1_000_001}),
            "cursor exceeds maximum",
        ),
        (
            "resolve_reference",
            json!({"instance":"missing-peer", "reference":"not-a-reference"}),
            "reference",
        ),
    ] {
        let output = f.invoke(name, input).await;
        assert!(!output.is_success());
        assert!(
            output.output().contains(expected),
            "{name}: {}",
            output.output()
        );
        assert!(!output.output().contains("peer connection not found"));
    }
}

#[tokio::test]
async fn production_coordinator_query_database_routes_to_local_when_omitted_or_exact_local() {
    let f = Fixture::new().await;
    let registry = f
        .runtime
        .coordinator_tool_registry(&f.caller, None)
        .await
        .unwrap();
    let query = registry.find_tool("query_database").unwrap();
    let local = query
        .run(
            json!({"sql":"SELECT 'local-selector-parity' AS source"}),
            f.ctx(),
        )
        .await;
    assert!(local.is_success(), "{}", local.output());
    assert!(local.output().contains("local-selector-parity"));
    let mut local_result: Value = serde_json::from_str(local.output()).unwrap();
    local_result["elapsed_ms"] = json!(0);
    for input in [
        json!({"sql":"SELECT 'local-selector-parity' AS source","instance":"local"}),
        json!({"sql":"SELECT 'local-selector-parity' AS source","instance":null}),
    ] {
        let output = query.run(input, f.ctx()).await;
        assert!(output.is_success(), "{}", output.output());
        let mut result: Value = serde_json::from_str(output.output()).unwrap();
        result["elapsed_ms"] = json!(0);
        assert_eq!(local_result, result);
    }
}
