use super::ServiceCapability;
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCatalog {
    pub title: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<ServiceCapability>,
    pub identity: String,
}

pub fn parse_catalog(body: &[u8], catalog_url: &Url) -> Result<ParsedCatalog, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid linkset json: {e}"))?;
    let linksets = value
        .get("linkset")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing linkset array".to_string())?;

    let mut title = None;
    let mut description = None;
    let mut capabilities = Vec::new();
    let mut seen = BTreeSet::new();
    let mut identity_parts = BTreeSet::new();

    capabilities.push(ServiceCapability::ApiCatalog {
        url: catalog_url.to_string(),
    });
    seen.insert(("api_catalog".to_string(), catalog_url.to_string()));

    for linkset in linksets.iter().filter_map(Value::as_object) {
        if title.is_none() {
            title = linkset
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if description.is_none() {
            description = linkset
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }

        for (rel, raw_links) in linkset {
            if matches!(rel.as_str(), "anchor" | "title" | "description") {
                continue;
            }
            for link in link_values(raw_links) {
                let Some(href) = link.get("href").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(url) = catalog_url.join(href) else {
                    continue;
                };
                if !matches!(url.scheme(), "http" | "https") {
                    tracing::debug!(
                        scheme = url.scheme(),
                        "dropping unsafe API catalog link scheme"
                    );
                    continue;
                }
                let identity_href = identity_href(&url);
                let url = url.to_string();
                if !seen.insert((rel.clone(), url.clone())) {
                    continue;
                }
                let link_title = link.get("title").and_then(Value::as_str).map(str::to_owned);
                let content_type = link
                    .get("type")
                    .or_else(|| link.get("content_type"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                identity_parts.insert((rel.clone(), identity_href, content_type.clone()));

                if title.is_none() {
                    title.clone_from(&link_title);
                }

                capabilities.push(classify_link(rel, url, link_title, content_type));
            }
        }
    }

    if identity_parts.is_empty() {
        return Err("empty linkset".to_string());
    }

    let identity = catalog_identity(&identity_parts);
    Ok(ParsedCatalog {
        title,
        description,
        capabilities,
        identity,
    })
}

fn identity_href(url: &Url) -> String {
    if is_loopback_url(url) {
        let port = url.port_or_known_default().unwrap_or(80);
        let mut normalized = format!("{}://loopback:{port}{}", url.scheme(), url.path());
        if let Some(query) = url.query() {
            normalized.push('?');
            normalized.push_str(query);
        }
        if let Some(fragment) = url.fragment() {
            normalized.push('#');
            normalized.push_str(fragment);
        }
        normalized
    } else {
        url.to_string()
    }
}

fn is_loopback_url(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    })
}

fn catalog_identity(parts: &BTreeSet<(String, String, Option<String>)>) -> String {
    let mut hasher = Sha256::new();
    for (rel, href, content_type) in parts {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(href.as_bytes());
        hasher.update(b"\0");
        if let Some(content_type) = content_type {
            hasher.update(content_type.as_bytes());
        }
        hasher.update(b"\0");
    }
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut out, "{byte:02x}").expect("write to string");
    }
    out
}

fn link_values(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    match value {
        Value::Array(values) => values.iter().filter_map(Value::as_object).collect(),
        Value::Object(object) => vec![object],
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Vec::new(),
    }
}

fn classify_link(
    rel: &str,
    url: String,
    title: Option<String>,
    content_type: Option<String>,
) -> ServiceCapability {
    let rel_lower = rel.to_ascii_lowercase();
    let type_lower = content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if rel_lower.contains("service-desc")
        || rel_lower.contains("openapi")
        || type_lower.contains("openapi")
    {
        ServiceCapability::OpenApi {
            url,
            title,
            content_type,
        }
    } else if rel_lower.contains("service-doc") || rel_lower.contains("doc") {
        ServiceCapability::Documentation { url, title }
    } else if rel_lower == "self" || type_lower.contains("text/html") {
        ServiceCapability::HtmlUi { url, title }
    } else {
        ServiceCapability::OtherLink {
            rel: rel.to_string(),
            url,
            title,
            content_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_catalog_links() {
        let url = Url::parse("http://127.0.0.1:8787/.well-known/api-catalog").unwrap();
        let parsed = parse_catalog(
            br#"{
              "linkset": [{
                "title": "debug-router",
                "service-desc": [{"href": "/openapi.json", "type": "application/openapi+json", "title": "OpenAPI"}],
                "service-doc": [{"href": "/docs", "title": "Docs"}],
                "self": [{"href": "/", "type": "text/html", "title": "UI"}]
              }]
            }"#,
            &url,
        )
        .unwrap();

        assert_eq!(parsed.title.as_deref(), Some("debug-router"));
        assert!(parsed
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ServiceCapability::OpenApi { .. })));
        assert!(parsed
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ServiceCapability::Documentation { .. })));
        assert!(parsed
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ServiceCapability::HtmlUi { .. })));
    }

    #[test]
    fn identity_ignores_mutable_display_labels() {
        let url = Url::parse("http://127.0.0.1:8787/.well-known/api-catalog").unwrap();
        let first = parse_catalog(
            br#"{
              "linkset": [{
                "title": "debug-router v1",
                "description": "booting",
                "service-doc": [{"href": "/docs", "title": "Docs v1"}]
              }]
            }"#,
            &url,
        )
        .unwrap();
        let second = parse_catalog(
            br#"{
              "linkset": [{
                "title": "debug-router v2",
                "description": "ready",
                "service-doc": [{"href": "/docs", "title": "Docs v2"}]
              }]
            }"#,
            &url,
        )
        .unwrap();

        assert_eq!(first.identity, second.identity);
    }

    #[test]
    fn identity_normalizes_reflected_loopback_absolute_hrefs() {
        let v4_url = Url::parse("http://127.0.0.1:8787/.well-known/api-catalog").unwrap();
        let v6_url = Url::parse("http://[::1]:8787/.well-known/api-catalog").unwrap();
        let v4 = parse_catalog(
            br#"{
              "linkset": [{
                "service-doc": [{"href": "http://127.0.0.1:8787/docs?view=api", "title": "Docs"}]
              }]
            }"#,
            &v4_url,
        )
        .unwrap();
        let v6 = parse_catalog(
            br#"{
              "linkset": [{
                "service-doc": [{"href": "http://[::1]:8787/docs?view=api", "title": "Docs"}]
              }]
            }"#,
            &v6_url,
        )
        .unwrap();

        assert_eq!(
            identity_href(&v4_url.join("http://127.0.0.1:8787/docs?view=api").unwrap()),
            identity_href(&v6_url.join("http://[::1]:8787/docs?view=api").unwrap())
        );
        assert_eq!(v4.identity, v6.identity);
    }

    #[test]
    fn rejects_catalogs_with_no_advertised_links() {
        let url = Url::parse("http://127.0.0.1:8787/.well-known/api-catalog").unwrap();
        let error = parse_catalog(
            br#"{
              "linkset": [{
                "title": "debug-router"
              }]
            }"#,
            &url,
        )
        .unwrap_err();

        assert_eq!(error, "empty linkset");
    }

    #[test]
    fn rejects_unsafe_link_schemes() {
        let url = Url::parse("http://127.0.0.1:8787/.well-known/api-catalog").unwrap();
        let parsed = parse_catalog(
            br#"{
              "linkset": [{
                "title": "debug-router",
                "service-doc": [{"href": "javascript:alert(1)", "title": "Bad"}],
                "self": [{"href": "/", "type": "text/html", "title": "UI"}]
              }]
            }"#,
            &url,
        )
        .unwrap();

        assert!(!parsed.capabilities.iter().any(|capability| matches!(
            capability,
            ServiceCapability::Documentation { url, .. } if url.starts_with("javascript:")
        )));
        assert!(parsed
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ServiceCapability::HtmlUi { .. })));
    }
}
