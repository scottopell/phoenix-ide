use crate::{Database, DbResult};
use sqlx::Row;

/// Immutable accepted SVG bytes and their conversation-owned presentation metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct SvgArtifact {
    pub artifact_id: String,
    pub conversation_id: String,
    pub title: String,
    pub description: String,
    pub width: f64,
    pub height: f64,
    pub bytes: Vec<u8>,
}

fn artifact_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SvgArtifact, sqlx::Error> {
    Ok(SvgArtifact {
        artifact_id: row.try_get("artifact_id")?,
        conversation_id: row.try_get("conversation_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        width: row.try_get("width")?,
        height: row.try_get("height")?,
        bytes: row.try_get("bytes")?,
    })
}

impl Database {
    /// Find an already committed publication before attempting to read staging again.
    ///
    /// # Errors
    /// Returns a database error if the lookup fails.
    pub async fn svg_artifact_for_invocation(
        &self,
        conversation_id: &str,
        tool_use_id: &str,
    ) -> DbResult<Option<SvgArtifact>> {
        sqlx::query("SELECT * FROM conversation_svg_artifacts WHERE conversation_id = ? AND tool_use_id = ?")
            .bind(conversation_id)
            .bind(tool_use_id)
            .fetch_optional(self.pool())
            .await?
            .as_ref().map(artifact_from_row)
            .transpose()
            .map_err(Into::into)
    }

    /// Commit bytes and ownership together; a repeated invocation returns its first snapshot.
    ///
    /// # Errors
    /// Returns a database error for missing owners, invalid metadata or failed persistence.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_svg_artifact(
        &self,
        conversation_id: &str,
        tool_use_id: &str,
        title: &str,
        description: &str,
        width: f64,
        height: f64,
        bytes: &[u8],
    ) -> DbResult<SvgArtifact> {
        let mut transaction = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO conversation_svg_artifacts
             (artifact_id, conversation_id, tool_use_id, title, description, width, height, bytes)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(conversation_id, tool_use_id) DO NOTHING",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(conversation_id)
        .bind(tool_use_id)
        .bind(title)
        .bind(description)
        .bind(width)
        .bind(height)
        .bind(bytes)
        .execute(&mut *transaction)
        .await?;
        let artifact = artifact_from_row(
            &sqlx::query("SELECT * FROM conversation_svg_artifacts WHERE conversation_id = ? AND tool_use_id = ?")
                .bind(conversation_id)
                .bind(tool_use_id)
                .fetch_one(&mut *transaction)
                .await?,
        )?;
        transaction.commit().await?;
        Ok(artifact)
    }

    /// Read a snapshot only within its owning transcript.
    ///
    /// # Errors
    /// Returns a database error if the lookup fails.
    pub async fn svg_artifact(
        &self,
        conversation_id: &str,
        artifact_id: &str,
    ) -> DbResult<Option<SvgArtifact>> {
        sqlx::query("SELECT * FROM conversation_svg_artifacts WHERE conversation_id = ? AND artifact_id = ?")
            .bind(conversation_id)
            .bind(artifact_id)
            .fetch_optional(self.pool())
            .await?
            .as_ref().map(artifact_from_row)
            .transpose()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_survives_worktree_removal_and_scope_retirement() {
        use crate::{ConvMode, ConvState, NonEmptyString};
        use phoenix_core::work_scope::{
            WorkScopeRetirementOutcome, WorkScopeRetirementPrecondition,
        };

        let db = Database::open_in_memory().await.unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let worktree_path = worktree.path().to_str().unwrap();
        let conversation = db
            .create_conversation_with_project(
                "retained-owner",
                "retained-owner",
                worktree_path,
                true,
                None,
                None,
                None,
                &ConvMode::Branch {
                    branch_name: NonEmptyString::new("chart-topic").unwrap(),
                    worktree_path: NonEmptyString::new(worktree_path).unwrap(),
                    base_branch: NonEmptyString::new("main").unwrap(),
                },
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        let scope = conversation.attached_work_scope_id.unwrap();
        let staging = worktree.path().join("chart.svg");
        std::fs::write(&staging, b"<svg/>").unwrap();
        let artifact = db
            .publish_svg_artifact(
                "retained-owner",
                "call",
                "Title",
                "Description",
                100.0,
                50.0,
                &std::fs::read(&staging).unwrap(),
            )
            .await
            .unwrap();
        db.update_conversation_state("retained-owner", &ConvState::Terminal)
            .await
            .unwrap();
        worktree.close().unwrap();
        assert!(!staging.exists());
        assert_eq!(
            db.retire_work_scope(
                WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(
                    scope,
                ),
                "owned worktree removed",
            )
            .await
            .unwrap(),
            WorkScopeRetirementOutcome::Retired,
        );
        assert!(db.get_conversation("retained-owner").await.is_ok());
        assert_eq!(
            db.svg_artifact("retained-owner", &artifact.artifact_id)
                .await
                .unwrap(),
            Some(artifact.clone()),
        );
        assert_eq!(
            db.svg_artifact_for_invocation("retained-owner", "call")
                .await
                .unwrap(),
            Some(artifact),
        );
    }

    #[tokio::test]
    async fn snapshot_survives_database_reopen_and_staging_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifacts.db");
        let staging = directory.path().join("chart.svg");
        std::fs::write(&staging, b"<svg/>").unwrap();
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        db.create_conversation("owner", "owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let artifact = db
            .publish_svg_artifact(
                "owner",
                "call",
                "Title",
                "Description",
                100.0,
                50.0,
                &std::fs::read(&staging).unwrap(),
            )
            .await
            .unwrap();
        std::fs::remove_file(staging).unwrap();
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(reopened.pool())
            .await
            .unwrap();
        assert_eq!(
            reopened
                .svg_artifact("owner", &artifact.artifact_id)
                .await
                .unwrap(),
            Some(artifact)
        );
    }

    #[tokio::test]
    async fn publication_is_immutable_owned_and_cascades_with_transcript() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("svg-owner", "svg-owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let first = db
            .publish_svg_artifact("svg-owner", "call", "Chart", "Bars", 100.0, 50.0, b"<svg/>")
            .await
            .unwrap();
        let replay = db
            .publish_svg_artifact(
                "svg-owner",
                "call",
                "Changed",
                "Changed",
                200.0,
                100.0,
                b"changed",
            )
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            db.svg_artifact_for_invocation("svg-owner", "call")
                .await
                .unwrap(),
            Some(first.clone())
        );
        assert!(db
            .svg_artifact("another-owner", &first.artifact_id)
            .await
            .unwrap()
            .is_none());
        let separate = db
            .publish_svg_artifact(
                "svg-owner",
                "call-2",
                "Chart",
                "Bars",
                100.0,
                50.0,
                b"<svg/>",
            )
            .await
            .unwrap();
        assert_ne!(first.artifact_id, separate.artifact_id);
        db.delete_conversation("svg-owner").await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_svg_artifacts")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn rejected_publication_leaves_no_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        assert!(db
            .publish_svg_artifact("missing", "call", "Chart", "Bars", 100.0, 50.0, b"<svg/>")
            .await
            .is_err());
        db.create_conversation("svg-owner", "svg-owner", "/tmp", true, None, None)
            .await
            .unwrap();
        for width in [0.0, -1.0, f64::NAN, f64::INFINITY, 16385.0] {
            assert!(db
                .publish_svg_artifact("svg-owner", "call", "Chart", "Bars", width, 50.0, b"<svg/>")
                .await
                .is_err());
        }
        assert!(db
            .svg_artifact_for_invocation("svg-owner", "call")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cancelled_transaction_rolls_back_snapshot_and_ownership_together() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("owner", "owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut transaction = db.pool().begin().await.unwrap();
        sqlx::query("INSERT INTO conversation_svg_artifacts VALUES ('id', 'owner', 'call', 'Title', 'Description', 100, 50, X'3c7376672f3e')")
            .execute(&mut *transaction).await.unwrap();
        // Dropping a cancelled publication's transaction schedules rollback.
        drop(transaction);
        assert!(db
            .svg_artifact_for_invocation("owner", "call")
            .await
            .unwrap()
            .is_none());
    }
}
