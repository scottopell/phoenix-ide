use super::ServiceCapability;
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;

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
                identity_parts.insert((
                    rel.clone(),
                    href.to_string(),
                    link_title.clone(),
                    content_type.clone(),
                ));

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

    let identity = catalog_identity(title.as_ref(), description.as_ref(), &identity_parts);
    Ok(ParsedCatalog {
        title,
        description,
        capabilities,
        identity,
    })
}

fn catalog_identity(
    title: Option<&String>,
    description: Option<&String>,
    parts: &BTreeSet<(String, String, Option<String>, Option<String>)>,
) -> String {
    let mut hasher = Sha256::new();
    if let Some(title) = title {
        hasher.update(b"title\0");
        hasher.update(title.as_bytes());
        hasher.update(b"\0");
    }
    if let Some(description) = description {
        hasher.update(b"description\0");
        hasher.update(description.as_bytes());
        hasher.update(b"\0");
    }
    for (rel, href, title, content_type) in parts {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(href.as_bytes());
        hasher.update(b"\0");
        if let Some(title) = title {
            hasher.update(title.as_bytes());
        }
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
