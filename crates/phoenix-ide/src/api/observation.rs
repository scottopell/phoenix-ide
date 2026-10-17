#[derive(Serialize)]
pub(crate) struct CoordinatorRouteResponse {
    pub(crate) coordinator_id: Option<String>,
}
use super::handlers::AppError;
use super::product_conversations::{
    read_scoped_product_conversation_list, CoordinatorHistoryQuery, OrdinarySnapshotResponse,
    SnapshotQuery,
};
use super::types::{
    ConversationRouteResponse, CoordinatorConversationSnapshotView,
    ProductConversationListResponse, ProductConversationListRow, ProductConversationRouteResponse,
    ProductConversationTranscriptRowView,
};
use crate::db::{Database, DbError};
use crate::observation_startup::{complete_stage, Stage};
use phoenix_core::domain::product_conversation::ProductConversationId;
use serde::Serialize;
use tracing::Instrument;

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(tag = "kind", content = "conversation", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub(crate) enum ObservationTargetResponse {
    Ordinary(Box<ProductConversationListRow>),
    Coordinator(Box<CoordinatorObservationTarget>),
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub(crate) struct CoordinatorObservationTarget {
    product_conversation_id: String,
    canonical_route: String,
    canonical_root: ProductConversationTranscriptRowView,
    latest_transcript_row_id: String,
    updated_at: String,
}

#[derive(Clone)]
enum RouteScope {
    Live,
    Product(ProductConversationId),
    Coordinator(ProductConversationId),
}

#[derive(Clone)]
pub(crate) struct ConversationRouteObservations {
    db: Database,
    scope: RouteScope,
}

impl ConversationRouteObservations {
    pub(crate) async fn for_target(db: Database, reference: &str) -> Result<Self, DbError> {
        let resolved = match db.resolve_ordinary_product_conversation(reference).await {
            Ok(resolved) => resolved,
            Err(DbError::ConversationNotFound(_)) => {
                let snapshot = db
                    .read_coordinator_product_conversation_snapshot(reference, None, None, 1)
                    .await?;
                return Ok(Self {
                    db,
                    scope: RouteScope::Coordinator(
                        snapshot.aggregate.product_conversation.id().clone(),
                    ),
                });
            }
            Err(error) => return Err(error),
        };
        let routes = Self {
            db,
            scope: RouteScope::Product(resolved.product_conversation_id),
        };
        routes.resolve_reference(reference).await?;
        Ok(routes)
    }

    #[tracing::instrument(name = "replay.admission", skip_all)]
    async fn resolve_reference(
        &self,
        reference: &str,
    ) -> Result<phoenix_db::ResolvedProductConversation, DbError> {
        complete_stage(Stage::Admission, async {
            if matches!(self.scope, RouteScope::Coordinator(_)) {
                return Err(DbError::ConversationNotFound(reference.to_owned()));
            }
            let mut resolved = complete_stage(
                Stage::ResolveOrdinary,
                self.db.resolve_ordinary_product_conversation(reference),
            )
            .instrument(tracing::info_span!("observation.resolve_ordinary"))
            .await?;
            if let RouteScope::Product(target) = &self.scope {
                if &resolved.product_conversation_id != target {
                    return Err(DbError::ConversationNotFound(reference.to_owned()));
                }
                if reference == resolved.product_conversation_id.as_str() {
                    resolved.requested_transcript_row_id = complete_stage(
                        Stage::GetAggregate,
                        self.db.get_ordinary_product_conversation(target),
                    )
                    .instrument(tracing::info_span!("observation.get_aggregate"))
                    .await?
                    .root
                    .conversation
                    .id;
                }
                self.require_member(&resolved.requested_transcript_row_id)
                    .await?;
            }
            Ok(resolved)
        })
        .await
    }

    #[tracing::instrument(name = "observation.require_member", skip_all)]
    pub(crate) async fn require_member(&self, id: &str) -> Result<(), DbError> {
        complete_stage(Stage::RequireMember, async {
            if let RouteScope::Coordinator(target) = &self.scope {
                let snapshot = self
                    .db
                    .read_coordinator_product_conversation_snapshot(id, None, None, 1)
                    .await?;
                if snapshot.aggregate.product_conversation.id() != target {
                    return Err(DbError::ConversationNotFound(id.to_owned()));
                }
                return Ok(());
            }
            let RouteScope::Product(target) = &self.scope else {
                return Ok(());
            };
            let resolved = complete_stage(
                Stage::MemberResolveOrdinary,
                self.db.resolve_ordinary_product_conversation(id),
            )
            .instrument(tracing::info_span!("observation.member.resolve_ordinary"))
            .await?;
            if &resolved.product_conversation_id != target {
                return Err(DbError::ConversationNotFound(id.to_owned()));
            }
            let aggregate = complete_stage(
                Stage::MemberGetAggregate,
                self.db.get_ordinary_product_conversation(target),
            )
            .instrument(tracing::info_span!("observation.member.get_aggregate"))
            .await?;
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
        })
        .await
    }

    pub(crate) async fn target(&self) -> Result<ObservationTargetResponse, AppError> {
        match &self.scope {
            RouteScope::Live => Err(AppError::NotFound("No observation target selected".into())),
            RouteScope::Product(target) => {
                let mut list = read_scoped_product_conversation_list(&self.db, Some(target))
                    .await?
                    .0;
                let row = list
                    .product_conversations
                    .pop()
                    .ok_or_else(|| AppError::NotFound(target.to_string()))?;
                Ok(ObservationTargetResponse::Ordinary(Box::new(row)))
            }
            RouteScope::Coordinator(target) => {
                let snapshot = self
                    .read_coordinator_history(
                        target.to_string(),
                        CoordinatorHistoryQuery {
                            before: None,
                            message_limit: Some(1),
                            open_id: None,
                        },
                    )
                    .await?
                    .0;
                Ok(ObservationTargetResponse::Coordinator(Box::new(
                    CoordinatorObservationTarget {
                        product_conversation_id: snapshot.product_conversation_id,
                        canonical_route: snapshot.canonical_route,
                        canonical_root: snapshot.canonical_root,
                        latest_transcript_row_id: snapshot.latest_transcript_row_id,
                        updated_at: snapshot.updated_at,
                    },
                )))
            }
        }
    }

    pub(crate) async fn list_product_conversations(
        &self,
    ) -> Result<axum::Json<ProductConversationListResponse>, AppError> {
        let target = match &self.scope {
            RouteScope::Live => None,
            RouteScope::Product(target) | RouteScope::Coordinator(target) => Some(target),
        };
        read_scoped_product_conversation_list(&self.db, target).await
    }

    pub(crate) async fn read_product_conversation(
        &self,
        reference: String,
        query: SnapshotQuery,
    ) -> Result<axum::Json<OrdinarySnapshotResponse>, AppError> {
        if !matches!(self.scope, RouteScope::Live) {
            self.resolve_reference(&reference)
                .await
                .map_err(product_route_db_to_app)?;
        }
        super::product_conversations::read_product_conversation(&self.db, reference, query).await
    }

    pub(crate) async fn read_coordinator_history(
        &self,
        reference: String,
        query: CoordinatorHistoryQuery,
    ) -> Result<axum::Json<CoordinatorConversationSnapshotView>, AppError> {
        match &self.scope {
            RouteScope::Live => {}
            RouteScope::Coordinator(target) => {
                let snapshot = self
                    .db
                    .read_coordinator_product_conversation_snapshot(&reference, None, None, 1)
                    .await
                    .map_err(product_route_db_to_app)?;
                if snapshot.aggregate.product_conversation.id() != target {
                    return Err(AppError::NotFound(reference));
                }
                self.require_member(&snapshot.requested_transcript_row_id)
                    .await
                    .map_err(product_route_db_to_app)?;
            }
            RouteScope::Product(_) => return Err(AppError::NotFound(reference)),
        }
        super::product_conversations::read_coordinator_history(&self.db, reference, query).await
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
        if let RouteScope::Product(_) = &self.scope {
            self.resolve_reference(conversation)
                .await
                .map_err(product_route_db_to_app)?;
            return Ok(CoordinatorRouteResponse {
                coordinator_id: None,
            });
        }
        if let RouteScope::Coordinator(_) = &self.scope {
            self.require_member(conversation)
                .await
                .map_err(product_route_db_to_app)?;
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
            coordinator_id: matched.then_some(root_id),
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
