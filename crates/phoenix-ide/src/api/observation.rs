#[derive(Serialize)]
pub(crate) struct CoordinatorRouteResponse {
    coordinator_id: Option<String>,
}
use super::handlers::AppError;
use super::types::{ConversationRouteResponse, ProductConversationRouteResponse};
use crate::db::{Database, DbError};
use phoenix_core::domain::product_conversation::ProductConversationId;
use serde::Serialize;

#[derive(Clone)]
enum RouteScope {
    Live,
    Product(ProductConversationId),
}

#[derive(Clone)]
pub(crate) struct ConversationRouteObservations {
    db: Database,
    scope: RouteScope,
}

impl ConversationRouteObservations {
    pub(crate) async fn for_target(db: Database, reference: &str) -> Result<Self, DbError> {
        let resolved = db.resolve_ordinary_product_conversation(reference).await?;
        let routes = Self {
            db,
            scope: RouteScope::Product(resolved.product_conversation_id),
        };
        routes.resolve_reference(reference).await?;
        Ok(routes)
    }

    async fn resolve_reference(
        &self,
        reference: &str,
    ) -> Result<phoenix_db::ResolvedProductConversation, DbError> {
        let mut resolved = self
            .db
            .resolve_ordinary_product_conversation(reference)
            .await?;
        if let RouteScope::Product(target) = &self.scope {
            if &resolved.product_conversation_id != target {
                return Err(DbError::ConversationNotFound(reference.to_owned()));
            }
            if reference == resolved.product_conversation_id.as_str() {
                resolved.requested_transcript_row_id = self
                    .db
                    .get_ordinary_product_conversation(target)
                    .await?
                    .root
                    .conversation
                    .id;
            }
            self.require_member(&resolved.requested_transcript_row_id)
                .await?;
        }
        Ok(resolved)
    }

    async fn require_member(&self, id: &str) -> Result<(), DbError> {
        let RouteScope::Product(target) = &self.scope else {
            return Ok(());
        };
        let resolved = self.db.resolve_ordinary_product_conversation(id).await?;
        if &resolved.product_conversation_id != target {
            return Err(DbError::ConversationNotFound(id.to_owned()));
        }
        let aggregate = self.db.get_ordinary_product_conversation(target).await?;
        let mut next = Some(aggregate.root.conversation.id.as_str());
        for _ in 0..aggregate.segments.len() {
            let Some(member) = next.and_then(|next| {
                aggregate
                    .segments
                    .iter()
                    .find(|segment| segment.transcript_row.conversation.id == next)
            }) else {
                break;
            };
            if member.transcript_row.conversation.id == id {
                return Ok(());
            }
            next = member
                .transcript_row
                .conversation
                .continued_in_conv_id
                .as_deref();
        }
        Err(DbError::ConversationNotFound(id.to_owned()))
    }

    pub(crate) async fn get_product_conversation_route(
        &self,
        reference: &str,
    ) -> Result<ProductConversationRouteResponse, AppError> {
        let db = &self.db;
        let resolved = self
            .resolve_reference(reference)
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

        self.require_member(&conversation.id)
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

        self.require_member(&conversation.id)
            .await
            .map_err(product_route_db_to_app)?;
        Ok(ConversationRouteResponse {
            id: conversation.id,
            slug: conversation.slug,
        })
    }

    pub(crate) async fn resolve_coordinator_route(
        &self,
        conversation: &str,
    ) -> Result<CoordinatorRouteResponse, AppError> {
        if matches!(self.scope, RouteScope::Product(_)) {
            self.resolve_reference(conversation)
                .await
                .map_err(product_route_db_to_app)?;
            return Ok(CoordinatorRouteResponse {
                coordinator_id: None,
            });
        }
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
        Self {
            db,
            scope: RouteScope::Live,
        }
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

    async fn scoped_fixture() -> (
        tempfile::TempDir,
        PathBuf,
        phoenix_db::Conversation,
        phoenix_db::Conversation,
        ConversationRouteObservations,
    ) {
        let (dir, path, db) = fixture().await;
        let root = db
            .create_conversation("a-root", "root-alias", "/fixture", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(row) => row,
            ContinueOutcome::AlreadyContinued(row) => panic!("already continued: {}", row.id),
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("parent state: {state_variant}")
            }
        };
        db.create_conversation("off-target", "off-alias", "/fixture", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "child",
            "child-alias",
            "/fixture",
            true,
            Some(&root.id),
            None,
        )
        .await
        .unwrap();
        db.create_conversation(
            "0-disconnected",
            "disconnected-alias",
            "/fixture",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        // Deliberately corrupt this disposable fixture, not a representable production state.
        sqlx::query("DROP TRIGGER conversations_reject_product_continuation_cycle_on_update")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = id WHERE id = '0-disconnected'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        for statement in [
            "DROP TRIGGER conversations_product_membership_is_immutable",
            "DROP TRIGGER conversations_require_single_product_root_on_update",
        ] {
            sqlx::query(statement).execute(db.pool()).await.unwrap();
        }
        sqlx::query(
            "UPDATE conversations SET product_conversation_id = ?, work_scope_id = (SELECT work_scope_id FROM conversations WHERE id = ?) WHERE id = '0-disconnected'",
        )
        .bind(root.product_conversation_id.as_str())
        .bind(&root.id)
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.resolve_ordinary_product_conversation(root.product_conversation_id.as_str())
                .await
                .unwrap()
                .requested_transcript_row_id,
            "0-disconnected"
        );
        assert_eq!(
            db.get_ordinary_product_conversation(&root.product_conversation_id)
                .await
                .unwrap()
                .root
                .conversation
                .id,
            root.id
        );
        let live = freeze(db, &path).await;
        (dir, path, root, successor, live)
    }

    #[tokio::test]
    async fn target_scope_checks_actual_topology_and_preserves_route_namespaces() {
        let (_dir, path, root, successor, live) = scoped_fixture().await;
        let original = std::fs::read(&path).unwrap();
        for target in [
            root.id.as_str(),
            "root-alias",
            root.product_conversation_id.as_str(),
            successor.id.as_str(),
            successor.slug.as_deref().unwrap(),
        ] {
            let routes = ConversationRouteObservations::for_target(live.db.clone(), target)
                .await
                .unwrap();
            for row in [&root, &successor] {
                assert_eq!(
                    routes.get_conversation_route(&row.id).await.unwrap().id,
                    row.id
                );
                assert_eq!(
                    routes
                        .get_by_slug_route(row.slug.as_deref().unwrap())
                        .await
                        .unwrap()
                        .id,
                    row.id
                );
                for reference in [
                    row.id.as_str(),
                    row.slug.as_deref().unwrap(),
                    root.product_conversation_id.as_str(),
                ] {
                    assert_eq!(
                        routes
                            .get_product_conversation_route(reference)
                            .await
                            .unwrap()
                            .transcript_row_id,
                        root.id
                    );
                    assert!(routes
                        .resolve_coordinator_route(reference)
                        .await
                        .unwrap()
                        .coordinator_id
                        .is_none());
                }
                assert!(matches!(
                    routes
                        .get_conversation_route(row.slug.as_deref().unwrap())
                        .await,
                    Err(AppError::NotFound(_))
                ));
                assert!(matches!(
                    routes.get_by_slug_route(&row.id).await,
                    Err(AppError::NotFound(_))
                ));
            }
            for (id, slug) in [
                ("off-target", "off-alias"),
                ("child", "child-alias"),
                ("0-disconnected", "disconnected-alias"),
            ] {
                assert!(matches!(
                    routes.get_conversation_route(id).await,
                    Err(AppError::NotFound(_))
                ));
                assert!(matches!(
                    routes.get_by_slug_route(slug).await,
                    Err(AppError::NotFound(_))
                ));
                for reference in [id, slug] {
                    assert!(matches!(
                        routes.get_product_conversation_route(reference).await,
                        Err(AppError::NotFound(_))
                    ));
                    assert!(matches!(
                        routes.resolve_coordinator_route(reference).await,
                        Err(AppError::NotFound(_))
                    ));
                }
            }
        }
        for reference in [
            "child",
            "child-alias",
            "0-disconnected",
            "disconnected-alias",
            "missing",
        ] {
            assert!(matches!(
                ConversationRouteObservations::for_target(live.db.clone(), reference).await,
                Err(DbError::ConversationNotFound(_))
            ));
        }
        live.db.pool().close().await;
        assert_unchanged(&path, &original);
    }
}
