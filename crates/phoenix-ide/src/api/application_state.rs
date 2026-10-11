use super::AppState;
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::net::SocketAddr;

pub(crate) use super::observation::ConversationRouteObservations;

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ApplicationState {
    Live(AppState),
    #[allow(dead_code)] // No observation startup is wired yet.
    Observation(ObservationApplication),
}

impl From<AppState> for ApplicationState {
    fn from(state: AppState) -> Self {
        Self::Live(state)
    }
}

#[derive(Clone, Copy)]
pub enum ObservationAccessPolicy {
    #[allow(dead_code)] // No observation startup is wired yet.
    LoopbackUnauthenticated,
}

#[derive(Clone)]
pub struct ObservationApplication {
    pub(crate) routes: ConversationRouteObservations,
    pub access_policy: ObservationAccessPolicy,
}

impl ObservationApplication {
    pub(crate) fn admit_peer(
        &self,
        peer: Option<SocketAddr>,
        headers: &HeaderMap,
    ) -> Result<(), ObservationAccessDenied> {
        if headers.contains_key("x-forwarded-for") || headers.contains_key("forwarded") {
            return Err(ObservationAccessDenied);
        }
        match self.access_policy {
            ObservationAccessPolicy::LoopbackUnauthenticated
                if peer.is_some_and(|peer| peer.ip().is_loopback()) =>
            {
                Ok(())
            }
            ObservationAccessPolicy::LoopbackUnauthenticated => Err(ObservationAccessDenied),
        }
    }
}

pub struct LiveState(pub AppState);

#[derive(Debug)]
pub struct UnavailableLiveCapability;

#[derive(Serialize)]
struct UnavailableCapabilityResponse {
    error_type: &'static str,
    capability: &'static str,
    error: &'static str,
}

impl IntoResponse for UnavailableLiveCapability {
    fn into_response(self) -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(UnavailableCapabilityResponse {
                error_type: "capability_unavailable",
                capability: "live_application",
                error: "Live application capability is unavailable",
            }),
        )
            .into_response()
    }
}

#[derive(Debug)]
pub struct ObservationAccessDenied;

impl IntoResponse for ObservationAccessDenied {
    fn into_response(self) -> Response {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error_type": "observation_access_denied",
                "error": "Observation access requires a direct loopback connection without forwarding headers"
            })),
        )
            .into_response()
    }
}

#[async_trait::async_trait]
impl FromRequestParts<ApplicationState> for LiveState {
    type Rejection = UnavailableLiveCapability;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &ApplicationState,
    ) -> Result<Self, Self::Rejection> {
        match state {
            ApplicationState::Live(state) => Ok(Self(state.clone())),
            ApplicationState::Observation(_) => Err(UnavailableLiveCapability),
        }
    }
}

#[async_trait::async_trait]
impl FromRequestParts<ApplicationState> for ConversationRouteObservations {
    type Rejection = ObservationAccessDenied;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApplicationState,
    ) -> Result<Self, Self::Rejection> {
        match state {
            ApplicationState::Live(state) => Ok(Self::new(state.db.clone())),
            ApplicationState::Observation(application) => {
                let peer = parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|peer| peer.0);
                application.admit_peer(peer, &parts.headers)?;
                Ok(application.routes.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests;
