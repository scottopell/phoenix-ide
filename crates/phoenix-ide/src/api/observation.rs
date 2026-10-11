#[derive(Serialize)]
pub(crate) struct CoordinatorRouteResponse {
    coordinator_id: Option<String>,
}
use super::handlers::AppError;
use super::types::{ConversationRouteResponse, ProductConversationRouteResponse};
use crate::db::{Database, DbError};
use serde::Serialize;

#[derive(Clone)]
pub(crate) struct ConversationRouteObservations {
    db: Database,
}

impl ConversationRouteObservations {
    pub(crate) async fn get_product_conversation_route(
        &self,
        reference: &str,
    ) -> Result<ProductConversationRouteResponse, AppError> {
        let db = &self.db;
        let resolved = db
            .resolve_ordinary_product_conversation(reference)
            .await
            .map_err(product_route_db_to_app)?;
        let conversation = db
            .get_ordinary_product_conversation(&resolved.product_conversation_id)
            .await
            .map_err(product_route_db_to_app)?
            .root
            .conversation;
        Ok(ProductConversationRouteResponse {
            transcript_row_id: conversation.id,
        })
    }

    pub(crate) async fn get_conversation_route(
        &self,
        id: &str,
    ) -> Result<ConversationRouteResponse, AppError> {
        let conversation = self
            .db
            .get_conversation(id)
            .await
            .map_err(product_route_db_to_app)?;

        Ok(ConversationRouteResponse {
            id: conversation.id,
            slug: conversation.slug,
        })
    }

    pub(crate) async fn get_by_slug_route(
        &self,
        slug: &str,
    ) -> Result<ConversationRouteResponse, AppError> {
        let conversation = self
            .db
            .get_conversation_by_slug(slug)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?;

        Ok(ConversationRouteResponse {
            id: conversation.id,
            slug: conversation.slug,
        })
    }

    pub(crate) async fn resolve_coordinator_route(
        &self,
        conversation: &str,
    ) -> Result<CoordinatorRouteResponse, AppError> {
        let Some(coordinator_id) = self
            .db
            .coordinator_conversation_id()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
        else {
            return Ok(CoordinatorRouteResponse {
                coordinator_id: None,
            });
        };
        let root_id = self
            .db
            .chain_root_of(&coordinator_id)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .unwrap_or_else(|| coordinator_id.clone());
        let members = self
            .db
            .chain_members_forward(&root_id)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let matched = if members.iter().any(|id| id == conversation) {
            true
        } else {
            match self.db.get_conversation_by_slug(conversation).await {
                Ok(candidate) => members.iter().any(|id| id == &candidate.id),
                Err(DbError::ConversationNotFound(_)) => false,
                Err(error) => return Err(AppError::Internal(error.to_string())),
            }
        };
        Ok(CoordinatorRouteResponse {
            coordinator_id: matched.then_some(coordinator_id),
        })
    }

    pub(crate) fn new(db: Database) -> Self {
        Self { db }
    }
}
pub(super) fn product_route_db_to_app(error: DbError) -> AppError {
    if let DbError::ConversationNotFound(id) = error {
        AppError::NotFound(id)
    } else {
        AppError::Internal(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_db::{ContinueOutcome, ConvState};
    use std::path::{Path, PathBuf};

    async fn fixture() -> (tempfile::TempDir, PathBuf, Database) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routes.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        (dir, path, db)
    }

    async fn freeze(db: Database, path: &Path) -> ConversationRouteObservations {
        let (busy, frames, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(frames, checkpointed);
        db.pool().close().await;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).unwrap();
        ConversationRouteObservations::new(
            Database::open_read_only(path.to_str().unwrap())
                .await
                .unwrap(),
        )
    }

    fn assert_unchanged(path: &Path, original: &[u8]) {
        assert_eq!(std::fs::read(path).unwrap(), original);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!PathBuf::from(format!("{}{suffix}", path.display())).exists());
        }
    }

    #[tokio::test]
    async fn immutable_fixture_resolves_id_slug_and_missing_without_creating_coordinator() {
        let (_dir, path, db) = fixture().await;
        let row = db
            .create_conversation("row-id", "canonical-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        let observations = freeze(db, &path).await;
        let original = std::fs::read(&path).unwrap();
        for route in [
            observations.get_conversation_route("row-id").await.unwrap(),
            observations
                .get_by_slug_route("canonical-slug")
                .await
                .unwrap(),
        ] {
            assert_eq!(
                serde_json::to_value(route).unwrap(),
                serde_json::json!({"id": "row-id", "slug": "canonical-slug"})
            );
        }
        assert!(matches!(
            observations.get_conversation_route("missing-id").await,
            Err(AppError::NotFound(message)) if message == "missing-id"
        ));
        assert!(matches!(
            observations.get_by_slug_route("missing-slug").await,
            Err(AppError::NotFound(message))
                if message == DbError::ConversationNotFound("missing-slug".to_owned()).to_string()
        ));
        for reference in [
            "row-id",
            "canonical-slug",
            row.product_conversation_id.as_str(),
        ] {
            let route = observations
                .get_product_conversation_route(reference)
                .await
                .unwrap();
            assert_eq!(route.transcript_row_id, "row-id");
        }
        assert!(matches!(
            observations.get_product_conversation_route("missing").await,
            Err(AppError::NotFound(_))
        ));
        for reference in ["row-id", "canonical-slug", "missing"] {
            assert_eq!(
                serde_json::to_value(
                    observations
                        .resolve_coordinator_route(reference)
                        .await
                        .unwrap()
                )
                .unwrap(),
                serde_json::json!({"coordinator_id": null})
            );
        }
        observations.db.pool().close().await;
        assert_unchanged(&path, &original);
    }

    #[tokio::test]
    async fn immutable_fixture_resolves_coordinator_chain_ids_and_slugs() {
        let (_dir, path, db) = fixture().await;
        let root = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "handoff".to_owned(),
            },
        )
        .await
        .unwrap();
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        db.create_conversation("ordinary", "ordinary-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        let observations = freeze(db, &path).await;
        let original = std::fs::read(&path).unwrap();
        for reference in [
            root.id.as_str(),
            root.slug.as_deref().unwrap(),
            successor.id.as_str(),
            successor.slug.as_deref().unwrap(),
        ] {
            let route = observations
                .resolve_coordinator_route(reference)
                .await
                .unwrap();
            assert_eq!(route.coordinator_id.as_deref(), Some(successor.id.as_str()));
        }
        for reference in ["ordinary", "ordinary-slug", "missing"] {
            assert!(observations
                .resolve_coordinator_route(reference)
                .await
                .unwrap()
                .coordinator_id
                .is_none());
        }
        observations.db.pool().close().await;
        assert_unchanged(&path, &original);
    }
}
