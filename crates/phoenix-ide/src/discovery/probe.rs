use super::linkset::parse_catalog;
use super::{DiscoveredService, DiscoveryConfidence, DiscoverySource, ServiceStatus};
use crate::discovery::config::DiscoveryConfig;
use chrono::Utc;
use futures::StreamExt;
use reqwest::{header, redirect::Policy, Client, Url};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeTarget {
    pub host: IpAddr,
    pub port: u16,
}

pub fn client(config: &DiscoveryConfig) -> Result<Client, reqwest::Error> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(config.probe_timeout)
        .connect_timeout(config.probe_timeout)
        .build()
}

pub async fn probe(
    client: &Client,
    config: &DiscoveryConfig,
    target: ProbeTarget,
) -> Result<DiscoveredService, String> {
    let catalog_url = catalog_url(target)?;
    let response = client
        .get(catalog_url.clone())
        .header(
            header::ACCEPT,
            "application/linkset+json, application/json;q=0.8",
        )
        .header(header::USER_AGENT, "phoenix-ide-local-discovery")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("status {}", response.status()));
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.contains("application/linkset+json") && !content_type.contains("json") {
        return Err(format!("unsupported content type {content_type:?}"));
    }

    let body = read_limited(response, config.body_limit_bytes).await?;
    let parsed = parse_catalog(&body, &catalog_url)?;
    let now = Utc::now();
    let base_url = base_url(target)?;
    let id = service_id(target);

    Ok(DiscoveredService {
        id,
        base_url: base_url.to_string(),
        host: target.host,
        port: target.port,
        title: parsed.title,
        description: parsed.description,
        capabilities: parsed.capabilities,
        first_seen_at: now,
        last_seen_at: now,
        status: ServiceStatus::Healthy,
        confidence: DiscoveryConfidence::ExplicitApiCatalog,
        source: DiscoverySource::LoopbackProbe,
    })
}

fn catalog_url(target: ProbeTarget) -> Result<Url, String> {
    base_url(target)?
        .join("/.well-known/api-catalog")
        .map_err(|e| format!("invalid catalog url: {e}"))
}

fn base_url(target: ProbeTarget) -> Result<Url, String> {
    let host = match target.host {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    Url::parse(&format!("http://{host}:{}", target.port))
        .map_err(|e| format!("invalid base url: {e}"))
}

fn service_id(target: ProbeTarget) -> String {
    format!("loopback:{}", target.port)
}

async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("read failed: {e}"))?;
        if body.len() + chunk.len() > limit {
            return Err(format!("response body exceeds {limit} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{service_id, ProbeTarget};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn dual_stack_loopback_uses_one_service_id_per_port() {
        let v4 = ProbeTarget {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8787,
        };
        let v6 = ProbeTarget {
            host: IpAddr::V6(Ipv6Addr::LOCALHOST),
            port: 8787,
        };

        assert_eq!(service_id(v4), service_id(v6));
    }
}
