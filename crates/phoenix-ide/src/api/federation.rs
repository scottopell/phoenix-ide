use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use phoenix_core::domain::instance_identity::{FederationCredentialVerifier, InstanceId};
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::auth::OwnerAuthenticated;
use super::AppState;

#[derive(Deserialize)]
pub struct IssueEnrollmentRequest {
    pub caller_instance_id: InstanceId,
    pub caller_display_name: String,
}

#[derive(Serialize)]
pub struct EnrollmentTransferBundle {
    pub receiver_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub token: String,
}

#[derive(Serialize)]
pub struct RevokeEnrollmentResponse {
    pub revoked: bool,
}

fn random_peer_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "phx_peer_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

pub async fn issue_enrollment(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<IssueEnrollmentRequest>,
) -> Response {
    let local_id = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.caller_instance_id == local_id || request.caller_display_name.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let token = random_peer_token();
    let verifier = FederationCredentialVerifier::from_bearer(token.as_bytes());
    if let Err(error) = state
        .db
        .replace_federation_enrollment(
            request.caller_instance_id,
            request.caller_display_name.trim(),
            &verifier,
        )
        .await
    {
        tracing::error!(%error, "failed to issue federation enrollment");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (
        StatusCode::CREATED,
        Json(EnrollmentTransferBundle {
            receiver_instance_id: local_id,
            caller_instance_id: request.caller_instance_id,
            token,
        }),
    )
        .into_response()
}

pub async fn revoke_enrollment(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(caller_instance_id): Path<InstanceId>,
) -> Response {
    match state
        .db
        .revoke_federation_enrollment(caller_instance_id)
        .await
    {
        Ok(revoked) => Json(RevokeEnrollmentResponse { revoked }).into_response(),
        Err(error) => {
            tracing::error!(%error, "failed to revoke federation enrollment");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_tokens_are_prefixed_random_credentials() {
        let first = random_peer_token();
        let second = random_peer_token();
        assert!(first.starts_with("phx_peer_"));
        assert_ne!(first, second);
        assert_eq!(first.len(), "phx_peer_".len() + 43);
    }
}
