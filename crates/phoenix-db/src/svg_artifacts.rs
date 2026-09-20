use crate::workflow::LocalAuthorityResult;
#[cfg(test)]
use crate::DbError;
use crate::{Database, DbResult};
use phoenix_svg::{SvgInvocationId, SvgPresentationMetadata, ValidatedSvg};
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

impl SvgArtifact {
    #[must_use]
    pub fn into_reference(self) -> phoenix_svg::SvgArtifactReference {
        phoenix_svg::SvgArtifactReference {
            artifact_id: self.artifact_id,
            conversation_id: self.conversation_id,
            title: self.title,
            description: self.description,
            width: self.width,
            height: self.height,
            validation: phoenix_svg::SvgValidationOutcome::AcceptedStaticSvg,
        }
    }
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

#[derive(Clone, Copy)]
enum PublicationCommit {
    Normal,
    #[cfg(test)]
    CommittedError,
    #[cfg(test)]
    RolledBackError,
    #[cfg(test)]
    CommittedLookupUnavailable,
}

impl PublicationCommit {
    async fn execute(self, transaction: sqlx::Transaction<'_, sqlx::Sqlite>) -> DbResult<()> {
        match self {
            Self::Normal => transaction.commit().await.map_err(Into::into),
            #[cfg(test)]
            Self::CommittedError | Self::CommittedLookupUnavailable => {
                transaction.commit().await?;
                Err(DbError::Serialization(
                    "injected commit acknowledgement failure".into(),
                ))
            }
            #[cfg(test)]
            Self::RolledBackError => {
                transaction.rollback().await?;
                Err(DbError::Serialization("injected commit failure".into()))
            }
        }
    }
}

impl Database {
    /// Find an already committed publication before attempting to read staging again.
    ///
    /// # Errors
    /// Returns a database error if the lookup fails.
    pub async fn svg_artifact_for_invocation(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
    ) -> DbResult<Option<SvgArtifact>> {
        sqlx::query("SELECT * FROM conversation_svg_artifacts WHERE conversation_id = ? AND assistant_message_id = ? AND tool_use_id = ?")
            .bind(conversation_id)
            .bind(&invocation.assistant_message_id)
            .bind(&invocation.tool_use_id)
            .fetch_optional(self.pool())
            .await?
            .as_ref().map(artifact_from_row)
            .transpose()
            .map_err(Into::into)
    }

    /// Commit validated bytes and metadata; classify an ambiguous commit by exact invocation.
    ///
    /// Raw bytes cannot cross the publication boundary:
    /// ```compile_fail
    /// async fn reject_raw_bytes(db: &phoenix_db::Database, id: &phoenix_svg::SvgInvocationId) {
    ///     let metadata = phoenix_svg::SvgPresentationMetadata::new("Chart", "Description").unwrap();
    ///     db.publish_svg_artifact("owner", id, &metadata, b"<svg/>").await;
    /// }
    /// ```
    /// Unvalidated presentation text cannot cross the publication boundary:
    /// ```compile_fail
    /// async fn reject_raw_metadata(db: &phoenix_db::Database, id: &phoenix_svg::SvgInvocationId, svg: &phoenix_svg::ValidatedSvg) {
    ///     db.publish_svg_artifact("owner", id, "Chart", svg).await;
    /// }
    /// ```
    pub async fn publish_svg_artifact(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        metadata: &SvgPresentationMetadata,
        svg: &ValidatedSvg,
    ) -> LocalAuthorityResult<DbResult<SvgArtifact>> {
        self.publish_svg_artifact_with_commit(
            conversation_id,
            invocation,
            metadata,
            svg,
            PublicationCommit::Normal,
        )
        .await
    }

    async fn publish_svg_artifact_with_commit(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        metadata: &SvgPresentationMetadata,
        svg: &ValidatedSvg,
        commit: PublicationCommit,
    ) -> LocalAuthorityResult<DbResult<SvgArtifact>> {
        let prepared: DbResult<_> = async {
            let mut transaction = self.pool().begin().await?;
            sqlx::query(
                "INSERT INTO conversation_svg_artifacts
                 (artifact_id, conversation_id, assistant_message_id, tool_use_id, title, description, width, height, bytes)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(conversation_id, assistant_message_id, tool_use_id) DO NOTHING",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(conversation_id)
            .bind(&invocation.assistant_message_id)
            .bind(&invocation.tool_use_id)
            .bind(metadata.title())
            .bind(metadata.description())
            .bind(svg.width())
            .bind(svg.height())
            .bind(svg.bytes())
            .execute(&mut *transaction)
            .await?;
            let artifact = artifact_from_row(
                &sqlx::query("SELECT * FROM conversation_svg_artifacts WHERE conversation_id = ? AND assistant_message_id = ? AND tool_use_id = ?")
                    .bind(conversation_id)
                    .bind(&invocation.assistant_message_id)
                    .bind(&invocation.tool_use_id)
                    .fetch_one(&mut *transaction)
                    .await?,
            )?;
            Ok((transaction, artifact))
        }.await;
        let (transaction, artifact) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error)),
        };
        let commit_result = commit.execute(transaction).await;
        match commit_result {
            Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(artifact)),
            Err(error) => {
                let lookup = self
                    .classify_svg_commit(conversation_id, invocation, commit)
                    .await;
                match lookup {
                    Ok(Some(artifact)) => {
                        LocalAuthorityResult::DurableFactEstablished(Ok(artifact))
                    }
                    Ok(None) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                    Err(_) => LocalAuthorityResult::DurableFactUnclassified,
                }
            }
        }
    }

    async fn classify_svg_commit(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        commit: PublicationCommit,
    ) -> DbResult<Option<SvgArtifact>> {
        let _ = commit;
        #[cfg(test)]
        if matches!(commit, PublicationCommit::CommittedLookupUnavailable) {
            return Err(DbError::Serialization(
                "injected classification failure".into(),
            ));
        }
        self.svg_artifact_for_invocation(conversation_id, invocation)
            .await
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

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"/>"#;

    fn invocation(tool_use_id: &str) -> SvgInvocationId {
        SvgInvocationId::new("assistant-message", tool_use_id)
    }

    fn validated() -> ValidatedSvg {
        phoenix_svg::validate(SVG).unwrap()
    }

    #[tokio::test]
    async fn ambiguous_commit_is_classified_by_exact_invocation() {
        for commit in [
            PublicationCommit::CommittedError,
            PublicationCommit::RolledBackError,
            PublicationCommit::CommittedLookupUnavailable,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            db.create_conversation("owner", "owner", "/tmp", true, None, None)
                .await
                .unwrap();
            let metadata = SvgPresentationMetadata::new("Chart", "Description").unwrap();
            db.publish_svg_artifact(
                "owner",
                &SvgInvocationId::new("other-assistant", "call"),
                &metadata,
                &validated(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
            let result = db
                .publish_svg_artifact_with_commit(
                    "owner",
                    &invocation("call"),
                    &metadata,
                    &validated(),
                    commit,
                )
                .await;
            match commit {
                PublicationCommit::CommittedError => {
                    let artifact = result.established().unwrap().unwrap();
                    assert_eq!(artifact.bytes, SVG);
                    assert_eq!(
                        db.svg_artifact_for_invocation("owner", &invocation("call"))
                            .await
                            .unwrap(),
                        Some(artifact)
                    );
                }
                PublicationCommit::RolledBackError => {
                    assert!(result.established().unwrap().is_err());
                    assert!(db
                        .svg_artifact_for_invocation("owner", &invocation("call"))
                        .await
                        .unwrap()
                        .is_none());
                }
                PublicationCommit::CommittedLookupUnavailable => {
                    assert!(matches!(
                        result,
                        LocalAuthorityResult::DurableFactUnclassified
                    ));
                    assert!(db
                        .svg_artifact_for_invocation("owner", &invocation("call"))
                        .await
                        .unwrap()
                        .is_some());
                }
                PublicationCommit::Normal => unreachable!(),
            }
        }
    }

    fn interrupted_svg_state(staging: &std::path::Path, cancelling: bool) -> crate::ConvState {
        use crate::{ConvState, ToolResult};
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, PresentSvgInput, ToolCall, ToolInput,
        };

        let input = PresentSvgInput {
            path: staging.to_str().unwrap().into(),
            title: "Chart".into(),
            description: "A retained chart".into(),
        };
        let assistant = AssistantMessage::new(
            "current-assistant".into(),
            vec![
                ContentBlock::tool_use("done", "think", serde_json::json!({"thoughts": "ready"})),
                ContentBlock::tool_use("svg", "present_svg", serde_json::to_value(&input).unwrap()),
            ],
            None,
            None,
        );
        let completed_results = vec![ToolResult::success("done".into(), "Ready".into())];
        if cancelling {
            ConvState::CancellingTool {
                tool_use_id: "svg".into(),
                skipped_tools: vec![],
                completed_results,
                assistant_message: assistant,
                pending_sub_agents: vec![],
            }
        } else {
            ConvState::ToolExecuting {
                current_tool: ToolCall::new("svg", ToolInput::PresentSvg(input)),
                remaining_tools: vec![],
                completed_results,
                pending_sub_agents: vec![],
                assistant_message: assistant,
            }
        }
    }

    #[tokio::test]
    async fn restart_recovers_only_committed_svg_invocation_after_staging_is_deleted() {
        use crate::MessageContent;

        for cancelling in [false, true] {
            for stored_assistant in [None, Some("older-assistant"), Some("current-assistant")] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("restart.db");
                let staging = directory.path().join("chart.svg");
                std::fs::write(&staging, SVG).unwrap();
                let db = Database::open(path.to_str().unwrap()).await.unwrap();
                crate::migrations::run_pending_migrations(db.pool())
                    .await
                    .unwrap();
                db.create_conversation("owner", "owner", "/tmp", true, None, None)
                    .await
                    .unwrap();
                let state = interrupted_svg_state(&staging, cancelling);
                db.update_conversation_state("owner", &state).await.unwrap();
                let published = if let Some(assistant_id) = stored_assistant {
                    Some(
                        db.publish_svg_artifact(
                            "owner",
                            &SvgInvocationId::new(assistant_id, "svg"),
                            &phoenix_svg::SvgPresentationMetadata::new("Chart", "A retained chart")
                                .unwrap(),
                            &validated(),
                        )
                        .await
                        .established()
                        .unwrap()
                        .unwrap(),
                    )
                } else {
                    None
                };
                std::fs::remove_file(&staging).unwrap();
                db.pool().close().await;
                let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
                crate::migrations::run_pending_migrations(reopened.pool())
                    .await
                    .unwrap();
                reopened.reset_all_to_idle().await.unwrap();
                reopened.reset_all_to_idle().await.unwrap();
                let messages = reopened.get_messages("owner").await.unwrap();
                assert_eq!(messages.len(), 3);
                assert!(messages.iter().any(|message| matches!(&message.content, MessageContent::Tool(tool) if tool.tool_use_id == "done" && tool.content == "Ready" && !tool.is_error)));
                let tool = messages
                    .iter()
                    .find_map(|message| {
                        if let MessageContent::Tool(tool) = &message.content {
                            (tool.tool_use_id == "svg").then_some(tool)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                if stored_assistant == Some("current-assistant") {
                    assert!(!tool.is_error);
                    let reference: phoenix_svg::SvgArtifactReference =
                        serde_json::from_str(&tool.content).unwrap();
                    assert_eq!(reference, published.unwrap().into_reference());
                    assert_eq!(
                        reopened
                            .svg_artifact("owner", &reference.artifact_id)
                            .await
                            .unwrap()
                            .unwrap()
                            .bytes,
                        SVG
                    );
                } else {
                    assert!(tool.is_error);
                    assert!(tool.content.contains("interrupted by server restart"));
                }
            }
        }
    }

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
        std::fs::write(&staging, SVG).unwrap();
        let artifact = db
            .publish_svg_artifact(
                "retained-owner",
                &invocation("call"),
                &phoenix_svg::SvgPresentationMetadata::new("Title", "Description").unwrap(),
                &phoenix_svg::validate(&std::fs::read(&staging).unwrap()).unwrap(),
            )
            .await
            .established()
            .unwrap()
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
            db.svg_artifact_for_invocation("retained-owner", &invocation("call"))
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
        std::fs::write(&staging, SVG).unwrap();
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
                &invocation("call"),
                &phoenix_svg::SvgPresentationMetadata::new("Title", "Description").unwrap(),
                &phoenix_svg::validate(&std::fs::read(&staging).unwrap()).unwrap(),
            )
            .await
            .established()
            .unwrap()
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
            .publish_svg_artifact(
                "svg-owner",
                &invocation("call"),
                &phoenix_svg::SvgPresentationMetadata::new("Chart", "Bars").unwrap(),
                &validated(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        let replay = db
            .publish_svg_artifact(
                "svg-owner",
                &invocation("call"),
                &phoenix_svg::SvgPresentationMetadata::new("Changed", "Changed").unwrap(),
                &phoenix_svg::validate(
                    br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"/>"#,
                )
                .unwrap(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            db.svg_artifact_for_invocation("svg-owner", &invocation("call"))
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
                &invocation("call-2"),
                &phoenix_svg::SvgPresentationMetadata::new("Chart", "Bars").unwrap(),
                &validated(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_ne!(first.artifact_id, separate.artifact_id);
        let later_assistant = SvgInvocationId::new("later-assistant-message", "call");
        assert!(db
            .svg_artifact_for_invocation("svg-owner", &later_assistant)
            .await
            .unwrap()
            .is_none());
        let reused_provider_id = db
            .publish_svg_artifact(
                "svg-owner",
                &later_assistant,
                &phoenix_svg::SvgPresentationMetadata::new("Later chart", "Bars").unwrap(),
                &validated(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_ne!(first.artifact_id, reused_provider_id.artifact_id);
        assert_eq!(
            db.svg_artifact_for_invocation("svg-owner", &later_assistant)
                .await
                .unwrap(),
            Some(reused_provider_id)
        );
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
            .publish_svg_artifact(
                "missing",
                &invocation("call"),
                &phoenix_svg::SvgPresentationMetadata::new("Chart", "Bars").unwrap(),
                &validated()
            )
            .await
            .established()
            .unwrap()
            .is_err());
        db.create_conversation("svg-owner", "svg-owner", "/tmp", true, None, None)
            .await
            .unwrap();
        for title in [String::new(), "x".repeat(201), "x\ny".into(), "   ".into()] {
            assert!(SvgPresentationMetadata::new(&title, "Bars").is_err());
        }
        assert!(db
            .svg_artifact_for_invocation("svg-owner", &invocation("call"))
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
        sqlx::query("INSERT INTO conversation_svg_artifacts VALUES ('id', 'owner', 'assistant-message', 'call', 'Title', 'Description', 100, 50, ?)")
            .bind(SVG)
            .execute(&mut *transaction).await.unwrap();
        // Dropping a cancelled publication's transaction schedules rollback.
        drop(transaction);
        assert!(db
            .svg_artifact_for_invocation("owner", &invocation("call"))
            .await
            .unwrap()
            .is_none());
    }
}
