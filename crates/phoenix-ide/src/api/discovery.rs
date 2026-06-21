use axum::{extract::State, Json};

use super::AppState;
use crate::discovery::DiscoveryServicesResponse;

pub async fn list_services(State(state): State<AppState>) -> Json<DiscoveryServicesResponse> {
    Json(DiscoveryServicesResponse {
        services: state.discovery.snapshot(),
    })
}
