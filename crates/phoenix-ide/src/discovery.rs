use chrono::{DateTime, Utc};
use serde::Serialize;
use std::net::IpAddr;
use std::sync::Arc;

pub mod config;
pub mod linkset;
pub mod probe;
pub mod registry;
pub mod supervisor;

pub use config::DiscoveryConfig;
pub use registry::DiscoveryRegistry;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredService {
    pub id: DiscoveredServiceId,
    pub base_url: String,
    pub host: IpAddr,
    pub port: u16,
    pub title: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<ServiceCapability>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub status: ServiceStatus,
    pub confidence: DiscoveryConfidence,
    pub source: DiscoverySource,
}

pub type DiscoveredServiceId = String;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Healthy,
    Stale,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryConfidence {
    ExplicitApiCatalog,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    LoopbackProbe,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServiceCapability {
    ApiCatalog {
        url: String,
    },
    OpenApi {
        url: String,
        title: Option<String>,
        content_type: Option<String>,
    },
    Documentation {
        url: String,
        title: Option<String>,
    },
    HtmlUi {
        url: String,
        title: Option<String>,
    },
    OtherLink {
        rel: String,
        url: String,
        title: Option<String>,
        content_type: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryServicesResponse {
    pub services: Vec<DiscoveredService>,
}

pub fn start(config: DiscoveryConfig) -> Arc<DiscoveryRegistry> {
    let registry = Arc::new(DiscoveryRegistry::new(config.clone()));
    if config.enabled {
        supervisor::start(config, registry.clone());
    }
    registry
}
