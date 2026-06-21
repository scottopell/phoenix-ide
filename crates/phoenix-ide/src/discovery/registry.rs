use super::{DiscoveredService, DiscoveredServiceId, ServiceStatus};
use crate::discovery::config::DiscoveryConfig;
use chrono::Utc;
use std::collections::BTreeMap;
use std::sync::RwLock;

pub struct DiscoveryRegistry {
    config: DiscoveryConfig,
    services: RwLock<BTreeMap<DiscoveredServiceId, DiscoveredService>>,
}

impl DiscoveryRegistry {
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            config,
            services: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn observe(&self, mut service: DiscoveredService) {
        let mut services = self.services.write().expect("discovery registry poisoned");
        if let Some(existing) = services.get_mut(&service.id) {
            service.first_seen_at = existing.first_seen_at;
            *existing = service;
        } else {
            services.insert(service.id.clone(), service);
        }
    }

    pub fn snapshot(&self) -> Vec<DiscoveredService> {
        self.refresh_lifecycle();
        let mut values: Vec<_> = self
            .services
            .read()
            .expect("discovery registry poisoned")
            .values()
            .cloned()
            .collect();
        values.sort_by(|a, b| {
            a.port
                .cmp(&b.port)
                .then_with(|| a.base_url.cmp(&b.base_url))
        });
        values
    }

    pub fn recent_ports(&self) -> Vec<u16> {
        self.refresh_lifecycle();
        let mut ports: Vec<_> = self
            .services
            .read()
            .expect("discovery registry poisoned")
            .values()
            .map(|service| service.port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    pub fn refresh_lifecycle(&self) {
        let now = Utc::now();
        let mut services = self.services.write().expect("discovery registry poisoned");
        services.retain(|_, service| {
            let Ok(age) = (now - service.last_seen_at).to_std() else {
                return true;
            };
            if age >= self.config.gone_after {
                return false;
            }
            if age >= self.config.stale_after {
                service.status = ServiceStatus::Stale;
            }
            true
        });
    }
}
