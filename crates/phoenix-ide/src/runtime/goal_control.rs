use phoenix_core::domain::{
    conversation_goal::GoalTimestampUs, product_conversation::ProductConversationId,
};
use phoenix_db::conversation_goals::control::{
    ControlCommand, ControlError, ControlReceipt, ControlRequest,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{api::auth::OwnerAuthenticated, db::Database};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalControlRequest {
    pub request_id: Uuid,
    pub command: ControlCommand,
    pub expected_control_version: u64,
}

#[derive(Default)]
pub struct GoalControlService {
    storage: Option<Database>,
}

#[derive(Debug, thiserror::Error)]
pub enum GoalControlServiceError {
    #[error("goal control is not installed")]
    Disabled,
    #[error(transparent)]
    Storage(#[from] crate::db::DbError),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    HostIdentity(#[from] uuid::Error),
    #[error("host clock is outside the supported control timestamp range")]
    Clock,
}

impl GoalControlService {
    /// Apply an owner-authenticated control request using the server's stable owner principal.
    /// The default service is disabled and no public route installs it.
    ///
    /// # Errors
    /// Rejects a disabled installation, unavailable owners, request conflicts and persistence failures.
    pub async fn apply(
        &self,
        _authenticated: OwnerAuthenticated,
        owner: ProductConversationId,
        request: GoalControlRequest,
    ) -> Result<ControlReceipt, GoalControlServiceError> {
        let db = self
            .storage
            .as_ref()
            .ok_or(GoalControlServiceError::Disabled)?;
        let principal_id = Uuid::parse_str(&db.instance_id().await?.to_string())?;
        let at = GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
            .map_err(|_| GoalControlServiceError::Clock)?;
        Ok(db
            .apply_goal_control(
                &ControlRequest {
                    owner,
                    principal_id,
                    request_id: request.request_id,
                    command: request.command,
                    expected_control_version: request.expected_control_version,
                },
                at,
            )
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::FromRequestParts, http::Request};
    use phoenix_db::conversation_goals::control::ControlOutcome;

    async fn fixture() -> (GoalControlService, ProductConversationId) {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("control-service-root", "control", "/tmp", true, None, None)
            .await
            .unwrap();
        let tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('conversation_goal_controls','conversation_goal_control_requests','conversation_goal_user_requests')",
        ).fetch_one(db.pool()).await.unwrap();
        assert_eq!(tables, 3);
        (
            GoalControlService { storage: Some(db) },
            conversation.product_conversation_id,
        )
    }

    fn request(id: Uuid, command: ControlCommand, version: u64) -> GoalControlRequest {
        GoalControlRequest {
            request_id: id,
            command,
            expected_control_version: version,
        }
    }

    #[tokio::test]
    async fn conversation_goal_control_service_real_receipt_retry_and_rejection_are_stable() {
        let (service, owner) = fixture().await;
        let first_id = Uuid::new_v4();
        let first = service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(first_id, ControlCommand::Stop, 0),
            )
            .await
            .unwrap();
        let second = service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(Uuid::new_v4(), ControlCommand::Release, 1),
            )
            .await
            .unwrap();
        assert!(
            matches!(second.result, ControlOutcome::Accepted { snapshot } if snapshot.version() == 2 && !snapshot.stopped())
        );
        let replay = service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(first_id, ControlCommand::Stop, 0),
            )
            .await
            .unwrap();
        assert_eq!(replay, first);
        let rejected_id = Uuid::new_v4();
        let rejected = service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(rejected_id, ControlCommand::Stop, 0),
            )
            .await
            .unwrap();
        assert!(
            matches!(&rejected.result, ControlOutcome::RejectedVersion { observed: Some(snapshot) } if snapshot.version() == 2)
        );
        service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(Uuid::new_v4(), ControlCommand::Stop, 2),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .apply(
                    OwnerAuthenticated,
                    owner.clone(),
                    request(rejected_id, ControlCommand::Stop, 0)
                )
                .await
                .unwrap(),
            rejected
        );
        assert!(matches!(
            service
                .apply(
                    OwnerAuthenticated,
                    owner,
                    request(first_id, ControlCommand::Release, 0)
                )
                .await,
            Err(GoalControlServiceError::Control(
                ControlError::RequestConflict
            ))
        ));
    }

    #[tokio::test]
    async fn conversation_goal_control_service_auth_extractor_rejects_guest_before_receipt_access()
    {
        let (service, owner) = fixture().await;
        let (mut parts, ()) = Request::new(()).into_parts();
        assert!(OwnerAuthenticated::from_request_parts(&mut parts, &())
            .await
            .is_err());
        parts
            .extensions
            .insert(crate::api::auth::PeerAuthenticated {
                enrollment_id: Uuid::new_v4(),
                caller_instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
                caller_display_name: "peer is not the host owner".into(),
            });
        assert!(OwnerAuthenticated::from_request_parts(&mut parts, &())
            .await
            .is_err());
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM conversation_goal_control_requests")
                .fetch_one(service.storage.as_ref().unwrap().pool())
                .await
                .unwrap();
        assert_eq!(count, 0);
        assert!(matches!(
            GoalControlService::default()
                .apply(
                    OwnerAuthenticated,
                    owner,
                    request(Uuid::new_v4(), ControlCommand::Stop, 0)
                )
                .await,
            Err(GoalControlServiceError::Disabled)
        ));
    }

    #[tokio::test]
    async fn conversation_goal_control_service_reauthorizes_owner_before_retry_lookup() {
        let (service, owner) = fixture().await;
        let request_id = Uuid::new_v4();
        service
            .apply(
                OwnerAuthenticated,
                owner.clone(),
                request(request_id, ControlCommand::Stop, 0),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=?")
            .bind(owner.as_str())
            .execute(service.storage.as_ref().unwrap().pool())
            .await
            .unwrap();
        assert!(matches!(
            service
                .apply(
                    OwnerAuthenticated,
                    owner,
                    request(request_id, ControlCommand::Stop, 0)
                )
                .await,
            Err(GoalControlServiceError::Control(
                ControlError::OwnerUnavailable
            ))
        ));
    }

    #[test]
    fn conversation_goal_control_service_input_cannot_choose_principal_or_actor() {
        for forged in ["principal_id", "actor", "owner"] {
            let mut body = serde_json::json!({"request_id": Uuid::new_v4(), "command":"stop", "expected_control_version":0});
            body[forged] = serde_json::json!("claimed-authority");
            assert!(serde_json::from_value::<GoalControlRequest>(body).is_err());
        }
    }
}
