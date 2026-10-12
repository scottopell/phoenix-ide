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
    Observation(ObservationApplication),
}

impl From<AppState> for ApplicationState {
    fn from(state: AppState) -> Self {
        Self::Live(state)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum CapabilityAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ApplicationCapabilities {
    pub persisted_member_read: CapabilityAvailability,
    pub live_stream: CapabilityAvailability,
    pub conversation_activation: CapabilityAvailability,
    pub user_input: CapabilityAvailability,
    pub telemetry: CapabilityAvailability,
    pub provider_catalog: CapabilityAvailability,
    pub host_resource_observations: CapabilityAvailability,
}

impl ApplicationState {
    fn capabilities(&self) -> ApplicationCapabilities {
        use CapabilityAvailability::{Available, Unavailable};
        let live = match self {
            Self::Live(_) => Available,
            Self::Observation(_) => Unavailable,
        };
        ApplicationCapabilities {
            persisted_member_read: Available,
            live_stream: live,
            conversation_activation: live,
            user_input: live,
            telemetry: live,
            provider_catalog: live,
            host_resource_observations: live,
        }
    }
}

#[async_trait::async_trait]
impl FromRequestParts<ApplicationState> for ApplicationCapabilities {
    type Rejection = ObservationAccessDenied;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApplicationState,
    ) -> Result<Self, Self::Rejection> {
        ConversationRouteObservations::from_request_parts(parts, state).await?;
        Ok(state.capabilities())
    }
}

#[derive(Clone, Copy)]
pub enum ObservationAccessPolicy {
    LoopbackUnauthenticated,
}

#[derive(Clone)]
pub struct ObservationApplication {
    routes: ConversationRouteObservations,
    access_policy: ObservationAccessPolicy,
}

impl ObservationApplication {
    pub(crate) async fn for_target(
        db: crate::db::Database,
        reference: &str,
    ) -> Result<Self, crate::db::DbError> {
        Ok(Self {
            routes: ConversationRouteObservations::for_target(db, reference).await?,
            access_policy: ObservationAccessPolicy::LoopbackUnauthenticated,
        })
    }

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
