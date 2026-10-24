use std::sync::Arc;

use phoenix_core::domain::conversation_goal::{
    GoalAdmissionObservation, GoalAgentSource, GoalAttribution, GoalDisposition, GoalExecution,
    GoalExecutionBinding, GoalFence, GoalId, GoalText, GoalVersion,
};
use phoenix_db::Database;

use crate::platform::PlatformCapability;
use crate::tools::mcp::McpClientManager;
use phoenix_llm::ModelRegistry;

use super::*;

fn test_runtime_manager(db: Database) -> Arc<RuntimeManager> {
    Arc::new(RuntimeManager::new(
        db,
        Arc::new(ModelRegistry::new_empty()),
        PlatformCapability::None {
            details: "test".to_string(),
        },
        Arc::new(McpClientManager::new()),
        None,
    ))
}

fn text(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}

fn at() -> GoalTimestampUs {
    GoalTimestampUs::parse(10).unwrap()
}

async fn fixture() -> (Arc<RuntimeManager>, GoalMutation) {
    let db = Database::open_in_memory().await.unwrap();
    let conversation = db
        .create_conversation("goal-root", "goal service", "/tmp", true, None, None)
        .await
        .unwrap();
    let host = GoalActor::GeneratedHost {
        action_id: text("test-command"),
    };
    let command = GoalMutation {
        owner: conversation.product_conversation_id,
        expected_version: GoalVersion::INITIAL,
        action_id: text("set-goal"),
        attribution: GoalAttribution {
            requested_by: host.clone(),
            applied_by: host,
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse("service-goal").unwrap(),
            objective: text("prepare source only"),
        },
    };
    (test_runtime_manager(db), command)
}

#[tokio::test]
async fn held_gates_refuse_instead_of_queuing_a_goal_mutation() {
    let (runtime, command) = fixture().await;
    let product = runtime
        .lock_product_message_admission(command.owner.as_str())
        .await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        apply(&runtime, &command, at()),
    )
    .await
    .expect("a held product admission gate must refuse, not wait");
    assert!(matches!(result, Err(GoalDbError::Busy)));
    drop(product);
    let acceptance = runtime.lock_message_acceptance("goal-root").await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        apply(&runtime, &command, at()),
    )
    .await
    .expect("a held acceptance gate must refuse, not wait");
    assert!(matches!(result, Err(GoalDbError::Busy)));
    assert!(get(&runtime, &command.owner).await.unwrap().is_none());
    drop(acceptance);
    apply(&runtime, &command, at()).await.unwrap();
}

#[tokio::test]
async fn accepted_retry_remains_readable_while_both_gates_are_held() {
    let (runtime, command) = fixture().await;
    let accepted = apply(&runtime, &command, at()).await.unwrap();
    let _product = runtime
        .lock_product_message_admission(command.owner.as_str())
        .await;
    let _acceptance = runtime.lock_message_acceptance("goal-root").await;
    let retry = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        apply(&runtime, &command, GoalTimestampUs::parse(20).unwrap()),
    )
    .await
    .expect("historical accepted retry must not acquire execution gates")
    .unwrap();
    assert_eq!(retry, accepted);
}

#[tokio::test]
async fn exact_report_path_does_not_wait_on_its_own_message_gates() {
    let (runtime, mut command) = fixture().await;
    let _product = runtime
        .lock_product_message_admission(command.owner.as_str())
        .await;
    let _acceptance = runtime.lock_message_acceptance("goal-root").await;
    let execution = GoalExecution::parse(1, 1).unwrap();
    command.attribution.requested_by = GoalActor::OwningAgent(GoalAgentSource {
        product_conversation_id: command.owner.clone(),
        transcript_id: text("goal-root"),
        execution,
        message_id: text("assistant-message"),
        tool_use_id: text("report-call"),
    });
    command.change = GoalChange::Report {
        binding: GoalExecutionBinding {
            owner: command.owner.clone(),
            execution,
            observation: GoalAdmissionObservation::Goal(GoalFence {
                owner: command.owner.clone(),
                goal_id: GoalId::parse("service-goal").unwrap(),
                version: GoalVersion::parse(1).unwrap(),
            }),
        },
        disposition: GoalDisposition::Complete(text("finished")),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        apply(&runtime, &command, at()),
    )
    .await
    .expect("report must reach the exact live-execution check without taking message wait gates");
    assert!(matches!(result, Err(GoalDbError::BindingConflict)));
}

#[tokio::test]
async fn get_reads_persisted_goal_without_chat_or_admission() {
    let (runtime, command) = fixture().await;
    assert!(get(&runtime, &command.owner).await.unwrap().is_none());
    let accepted = apply(&runtime, &command, at()).await.unwrap();
    assert_eq!(
        get(&runtime, &command.owner).await.unwrap(),
        Some(accepted.snapshot)
    );
    let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
    assert_eq!(messages, 0);
}
