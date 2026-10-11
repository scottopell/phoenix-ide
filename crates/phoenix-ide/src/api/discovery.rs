use super::application_state::LiveState;
use axum::extract::Json;

use crate::discovery::DiscoveryServicesResponse;

pub async fn list_services(LiveState(state): LiveState) -> Json<DiscoveryServicesResponse> {
    Json(DiscoveryServicesResponse {
        services: state.discovery.snapshot(),
    })
}
