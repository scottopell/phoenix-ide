use super::*;
use phoenix_core::domain::{
    db_schema::{InputOrigin, MessageContent},
    llm_types::ContentBlock,
    sm_event::{DirectTurnTurnId, PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity},
};
use phoenix_workflow::{AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp};

struct Fixture {
    db: crate::db::Database,
    runtime: Arc<RuntimeManager>,
    caller: String,
    owner: ProductConversationId,
    turn_id: DirectTurnTurnId,
}

impl Fixture {
    async fn new() -> Self {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let caller = db.get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default()).await.unwrap().id;
        let target = db.create_conversation("goal-native-target", "goal-native-target", "/tmp", true, None, None).await.unwrap();
        let owner = target.product_conversation_id;
        let accepted = db.workflow_repository().accept_authoritative_turn(&runtime_input(&caller, "native-goal-source")).await.unwrap();
        let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else { panic!("fresh runtime turn"); };
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(), Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None { details: "Goal native test".into() },
            Arc::new(crate::tools::mcp::McpClientManager::new()), None,
        ));
        Self { db, runtime, caller, owner, turn_id: DirectTurnTurnId(turn_id.0) }
    }

    fn ctx(&self) -> ToolContext {
        super::super::super::tests::context(&self.caller).with_direct_turn_id(self.turn_id)
    }

    async fn invoke(&self, name: &str, input: Value) -> ToolOutput {
        let call_id = uuid::Uuid::new_v4().to_string();
        let message_id = uuid::Uuid::new_v4().to_string();
        self.db.add_message(&message_id, &self.caller, &MessageContent::Agent(vec![
            ContentBlock::ToolUse { id: call_id.clone(), name: name.into(), input: input.clone() },
        ]), None, None).await.unwrap();
        let tool = tools(self.runtime.clone()).into_iter().find(|t| t.name() == name).unwrap();
        tool.run(input, self.ctx().with_tool_use_id(&call_id).with_svg_assistant_message_id(&message_id)).await
    }

    fn target(&self) -> String { format!("@conv:{}", self.owner.as_str()) }
}

fn value(output: ToolOutput) -> Value {
    assert!(!output.is_error, "{}", output.content.as_str());
    serde_json::from_str(output.content.as_str()).unwrap()
}

#[tokio::test]
async fn native_goal_discovery_prepare_apply_read_and_replay_without_dispatch() {
    let f = Fixture::new().await;
    let names: Vec<_> = tools(f.runtime.clone()).iter().map(|t| t.name().to_string()).collect();
    assert_eq!(names, ["read_goal", "prepare_goal", "apply_goal"]);
    let prepared = value(f.invoke("prepare_goal", json!({"target":f.target(),"action":{"operation":"create","objective":"Native goal journey"}})).await);
    assert_eq!(prepared["expected_version"], 0);
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
    let action = json!({"action_reference":prepared["action_reference"]});
    let accepted = value(f.invoke("apply_goal", action.clone()).await);
    assert_eq!(accepted["receipt"]["result"]["outcome"], "accepted");
    let replayed = value(f.invoke("apply_goal", action).await);
    assert_eq!(accepted, replayed);
    let read = value(f.invoke("read_goal", json!({"target":f.target()})).await);
    assert_eq!(read["history"].as_array().unwrap().len(), 1);
    assert_eq!(read["execution_state_included"], false);
    let workflows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_workflow_sessions").fetch_one(f.db.pool()).await.unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns").fetch_one(f.db.pool()).await.unwrap();
    let queue: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversation_queued_messages").fetch_one(f.db.pool()).await.unwrap();
    assert_eq!((workflows, turns, queue), (1, 1, 0));
}

#[tokio::test]
async fn native_goal_tools_refuse_raw_ids_authority_override_and_missing_turn() {
    let f = Fixture::new().await;
    for input in [json!({"target":f.owner.as_str()}), json!({"target":f.target(),"caller":"coordinator"}), json!({"target":f.target(),"after_version":-1})] {
        assert!(f.invoke("read_goal", input).await.is_error);
    }
    assert!(f.invoke("apply_goal", json!({"action_reference":"invented"})).await.is_error);
    assert!(f.invoke("apply_goal", json!({"action_reference":uuid::Uuid::new_v4(),"target":f.target()})).await.is_error);
    let prepare = tools(f.runtime.clone()).into_iter().find(|t| t.name()=="prepare_goal").unwrap();
    let missing = prepare.run(json!({"target":f.target(),"action":{"operation":"create","objective":"refused"}}), super::super::super::tests::context(&f.caller)).await;
    assert!(missing.is_error);
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
}

#[tokio::test]
async fn native_goal_tools_recheck_coordinator_role_on_every_operation() {
    let f = Fixture::new().await;
    let ordinary = "goal-native-target";
    for name in ["read_goal", "prepare_goal", "apply_goal"] {
        let tool = tools(f.runtime.clone()).into_iter().find(|t| t.name()==name).unwrap();
        let input = match name {
            "read_goal" => json!({"target":f.target()}),
            "prepare_goal" => json!({"target":f.target(),"action":{"operation":"create","objective":"refused"}}),
            _ => json!({"action_reference":uuid::Uuid::new_v4()}),
        };
        let output = tool.run(input, super::super::super::tests::context(&ordinary).with_direct_turn_id(f.turn_id)).await;
        assert!(output.is_error, "{name} accepted an ordinary caller");
    }
    assert!(f.db.conversation_goal(&f.owner).await.unwrap().is_none());
}
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