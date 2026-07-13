use std::{str::FromStr, time::Duration};

use chrono::{TimeZone, Utc};
use phoenix_workflow::creation_profile::{
    AuthoritativeCreationOracle, AuthoritativeCreationStage, AuthoritativeCreationStatus,
    CreationIntent, CreationKind, CreationProjectionStatus,
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};

use super::*;
use crate::run_pending_migrations;

async fn pool() -> SqlitePool {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE conversations (id TEXT PRIMARY KEY, slug TEXT UNIQUE, cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01', archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', steering_queue TEXT NOT NULL DEFAULT '[]');
         CREATE TABLE messages (message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sequence_id INTEGER NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, display_data TEXT, usage_data TEXT, created_at TEXT NOT NULL, FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE);
         INSERT INTO conversations (id, slug) VALUES ('conv-shadow', 'conv-shadow');",
    )
    .execute(&pool)
    .await
    .unwrap();
    run_pending_migrations(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO conversation_creation_jobs (id, conversation_id, message_id, status, stage, attempt, generation, intent_json, accepted_at, created_at, updated_at) VALUES ('job-shadow', 'conv-shadow', NULL, 'accepted', 'validate_intent', 0, 0, '{}', '2025-01-01', '2025-01-01', '2025-01-01')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

fn oracle() -> AuthoritativeCreationOracle {
    AuthoritativeCreationOracle {
        intent: CreationIntent {
            job_id: "job-shadow".to_owned(),
            conversation_id: "conv-shadow".to_owned(),
            idempotency_key: "key".to_owned(),
            repository_path: "/repo".to_owned(),
            worktree_path: "/repo/wt".to_owned(),
            branch_name: "shadow".to_owned(),
            initial_text: "authoritative semantic bytes must not be copied".to_owned(),
            attachment_ids: vec!["attachment-secret".to_owned()],
            kind: CreationKind::InitialTurn {
                message_id: "message-secret".to_owned(),
            },
        },
        status: AuthoritativeCreationStatus::Accepted,
        stage: AuthoritativeCreationStage::ValidateIntent,
        attempt: 0,
        generation: 0,
    }
}

fn config() -> CreationShadowPersistence {
    CreationShadowPersistence::Enabled(CreationShadowConfig {
        shadow_workflow_id: "creation-shadow".to_owned(),
        authoritative_anchor_workflow_id: "creation-authoritative-anchor".to_owned(),
    })
}

async fn row_count(pool: &SqlitePool, table: &'static str) -> i64 {
    let sql = match table {
        "creation_shadow_bindings" => "SELECT COUNT(*) FROM creation_shadow_bindings",
        "workflow_claims" => "SELECT COUNT(*) FROM workflow_claims",
        "workflow_attempts" => "SELECT COUNT(*) FROM workflow_attempts",
        "workflow_effects" => "SELECT COUNT(*) FROM workflow_effects",
        "workflow_transitions" => "SELECT COUNT(*) FROM workflow_transitions",
        "creation_shadow_effect_predictions" => {
            "SELECT COUNT(*) FROM creation_shadow_effect_predictions"
        }
        "creation_shadow_divergences" => "SELECT COUNT(*) FROM creation_shadow_divergences",
        _ => panic!("unlisted test table"),
    };
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn disabled_is_structural_and_writes_nothing() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let outcome = CreationShadowAdapter::new(&repo, &CreationShadowPersistence::Disabled)
        .persist_after_authoritative_commit(
            &oracle(),
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
            Utc.timestamp_opt(1_000, 0).single().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(outcome, CreationShadowPersistOutcome::Disabled);
    assert_eq!(row_count(&pool, "creation_shadow_bindings").await, 0);
}

#[tokio::test]
async fn persists_real_bounded_shadow_graph_without_authority_or_semantic_byte_duplication() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let persistence = config();
    let adapter = CreationShadowAdapter::new(&repo, &persistence);
    assert_eq!(
        adapter
            .persist_after_authoritative_commit(
                &oracle(),
                CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
                Utc.timestamp_opt(1_000, 0).single().unwrap(),
            )
            .await
            .unwrap(),
        CreationShadowPersistOutcome::Created
    );
    assert_eq!(
        adapter
            .persist_after_authoritative_commit(
                &oracle(),
                CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
                Utc.timestamp_opt(1_001, 0).single().unwrap(),
            )
            .await
            .unwrap(),
        CreationShadowPersistOutcome::Updated
    );

    let workflow = sqlx::query("SELECT authority, execution_mode, authoritative_workflow_id, snapshot_payload FROM workflows WHERE id = 'creation-shadow'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(workflow.get::<String, _>("authority"), "legacy_protocol");
    assert_eq!(workflow.get::<String, _>("execution_mode"), "shadow");
    assert_eq!(
        workflow.get::<String, _>("authoritative_workflow_id"),
        "creation-authoritative-anchor"
    );
    assert_eq!(workflow.get::<String, _>("snapshot_payload"), "{}");

    let selection = sqlx::query("SELECT accepting, runtime_acceptance_enabled, external_acceptance_enabled FROM workflow_protocol_selections WHERE id = ?1")
        .bind(SELECTION_ID).fetch_one(&pool).await.unwrap();
    assert_eq!(selection.get::<i64, _>("accepting"), 0);
    assert_eq!(selection.get::<i64, _>("runtime_acceptance_enabled"), 0);
    assert_eq!(selection.get::<i64, _>("external_acceptance_enabled"), 0);
    assert_eq!(row_count(&pool, "workflow_claims").await, 0);
    assert_eq!(row_count(&pool, "workflow_attempts").await, 0);
    assert_eq!(row_count(&pool, "workflow_effects").await, 7);
    assert_eq!(row_count(&pool, "workflow_transitions").await, 1);
    assert_eq!(
        row_count(&pool, "creation_shadow_effect_predictions").await,
        7
    );
    let executable: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'creation-shadow' AND status <> 'blocked'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(executable, 0);
    let leaked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE snapshot_payload LIKE '%semantic bytes%' OR snapshot_payload LIKE '%attachment-secret%' OR snapshot_payload LIKE '%message-secret%'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(leaked, 0);
}

#[tokio::test]
async fn divergence_lifecycle_is_bounded_and_authoritative_job_never_mutates() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let persistence = config();
    let adapter = CreationShadowAdapter::new(&repo, &persistence);
    let before = sqlx::query("SELECT status, stage, attempt, generation, intent_json, updated_at FROM conversation_creation_jobs WHERE id = 'job-shadow'")
        .fetch_one(&pool).await.unwrap();

    for second in 0..3 {
        adapter
            .persist_after_authoritative_commit(
                &oracle(),
                CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Ready),
                Utc.timestamp_opt(2_000 + second, 0).single().unwrap(),
            )
            .await
            .unwrap();
    }
    assert_eq!(row_count(&pool, "creation_shadow_divergences").await, 1);
    adapter
        .persist_after_authoritative_commit(
            &oracle(),
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
            Utc.timestamp_opt(2_010, 0).single().unwrap(),
        )
        .await
        .unwrap();
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM creation_shadow_divergences WHERE resolved_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 0);
    adapter
        .persist_after_authoritative_commit(
            &oracle(),
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Ready),
            Utc.timestamp_opt(2_020, 0).single().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(row_count(&pool, "creation_shadow_divergences").await, 2);

    let after = sqlx::query("SELECT status, stage, attempt, generation, intent_json, updated_at FROM conversation_creation_jobs WHERE id = 'job-shadow'")
        .fetch_one(&pool).await.unwrap();
    for column in ["status", "stage", "intent_json", "updated_at"] {
        assert_eq!(
            before.get::<String, _>(column),
            after.get::<String, _>(column)
        );
    }
    for column in ["attempt", "generation"] {
        assert_eq!(before.get::<i64, _>(column), after.get::<i64, _>(column));
    }
}

#[tokio::test]
async fn independent_user_capabilities_record_divergence() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let mut capabilities = creation_profile::project_authoritative_creation(&oracle()).capabilities;
    capabilities.cancel = CapabilityAvailability::Forbidden;
    CreationShadowAdapter::new(&repo, &config())
        .persist_after_authoritative_commit(
            &oracle(),
            CreationShadowEvidence::UserProjection {
                status: CreationProjectionStatus::Provisioning,
                capabilities,
            },
            Utc.timestamp_opt(2_050, 0).single().unwrap(),
        )
        .await
        .unwrap();

    let divergence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM creation_shadow_divergences WHERE evidence_identity = 'capability_cancel' AND resolved_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(divergence, 1);
}

#[tokio::test]
async fn stale_status_or_stage_snapshot_is_rejected() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let persistence = config();
    let adapter = CreationShadowAdapter::new(&repo, &persistence);
    let stale = oracle();

    sqlx::query("UPDATE conversation_creation_jobs SET status = 'ready', stage = 'finalize', completed_at = '2025-01-02' WHERE id = 'job-shadow'")
        .execute(&pool)
        .await
        .unwrap();

    assert!(adapter
        .persist_after_authoritative_commit(
            &stale,
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
            Utc.timestamp_opt(2_100, 0).single().unwrap(),
        )
        .await
        .is_err());
    assert_eq!(row_count(&pool, "creation_shadow_bindings").await, 0);
}

#[tokio::test]
async fn hard_delete_cascades_through_binding_to_both_workflow_graphs() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let persistence = config();
    CreationShadowAdapter::new(&repo, &persistence)
        .persist_after_authoritative_commit(
            &oracle(),
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
            Utc.timestamp_opt(2_200, 0).single().unwrap(),
        )
        .await
        .unwrap();

    sqlx::query("DELETE FROM conversations WHERE id = 'conv-shadow'")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(row_count(&pool, "creation_shadow_bindings").await, 0);
    let workflows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflows WHERE id IN ('creation-shadow', 'creation-authoritative-anchor')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(workflows, 0);
    assert_eq!(row_count(&pool, "workflow_effects").await, 0);
    assert_eq!(row_count(&pool, "workflow_transitions").await, 0);
}

#[tokio::test]
async fn cleanup_status_persists_compensation_graph() {
    let pool = pool().await;
    sqlx::query(
        "UPDATE conversation_creation_jobs SET status = 'cancelling' WHERE id = 'job-shadow'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut cleanup = oracle();
    cleanup.status = AuthoritativeCreationStatus::Cancelling;
    let repo = WorkflowRepository::new(pool.clone());
    CreationShadowAdapter::new(&repo, &config())
        .persist_after_authoritative_commit(
            &cleanup,
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Cancelled),
            Utc.timestamp_opt(2_300, 0).single().unwrap(),
        )
        .await
        .unwrap();

    let compensation: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'creation-shadow' AND role = 'compensation'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(compensation, 5);
}

#[tokio::test]
async fn failure_is_returned_without_mutating_authoritative_job() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let mut stale = oracle();
    stale.generation = 1;
    let error = CreationShadowAdapter::new(&repo, &config())
        .persist_after_authoritative_commit(
            &stale,
            CreationShadowEvidence::ProjectionStatus(CreationProjectionStatus::Provisioning),
            Utc.timestamp_opt(3_000, 0).single().unwrap(),
        )
        .await;
    assert!(error.is_err());
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM conversation_creation_jobs WHERE id = 'job-shadow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(generation, 0);
    assert_eq!(row_count(&pool, "creation_shadow_bindings").await, 0);
}
