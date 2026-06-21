use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub hosts: Vec<IpAddr>,
    pub ports: Vec<u16>,
    pub probe_timeout: Duration,
    pub max_concurrent_probes: usize,
    pub sweep_interval: Duration,
    pub recent_interval: Duration,
    pub body_limit_bytes: usize,
    pub stale_after: Duration,
    pub gone_after: Duration,
}

impl DiscoveryConfig {
    pub fn from_env() -> Self {
        let default_enabled = cfg!(debug_assertions);
        let enabled = std::env::var("PHOENIX_DISCOVERY_ENABLED")
            .ok()
            .and_then(|v| parse_bool(&v))
            .unwrap_or(default_enabled);

        Self {
            enabled,
            hosts: vec![
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ],
            ports: default_ports(),
            probe_timeout: Duration::from_millis(200),
            max_concurrent_probes: 32,
            sweep_interval: Duration::from_secs(60),
            recent_interval: Duration::from_secs(15),
            body_limit_bytes: 64 * 1024,
            stale_after: Duration::from_secs(90),
            gone_after: Duration::from_secs(180),
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn default_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    for (start, end) in [
        (3000, 3010),
        (5000, 5010),
        (5173, 5180),
        (8000, 8099),
        (9000, 9099),
    ] {
        ports.extend(start..=end);
    }
    ports
}
