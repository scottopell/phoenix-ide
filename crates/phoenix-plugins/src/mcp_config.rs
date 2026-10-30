//! `mcp.json` parsing and per-server validation (REQ-AP-004, REQ-AP-006,
//! REQ-AP-012).
//!
//! Document-level failure (bad JSON, bad structure, version mismatch with
//! the manifest, unsupported version) disables the plugin's MCP
//! contribution. Per-server failure skips that server only. The document
//! validates each server independently — `ServerValidation` is the
//! per-server failure boundary.

use std::collections::BTreeMap;

use serde_json::Value;

/// The canonical 1.1.0 MCP schema identifier (REQ-AP-012).
pub const MCP_SCHEMA_URL: &str = "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json";

/// One validated server entry: exactly one closed variant, placeholders
/// not yet expanded. The deprecated `sse` transport never reaches this
/// type (it is a skip with a diagnostic, per REQ-AP-004 / REQ-MCP-019).
#[derive(Debug, Clone, PartialEq)]
pub enum ServerSpec {
    Stdio {
        server: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        /// None means the plugin root (the default working directory).
        cwd: Option<String>,
    },
    StreamableHttp {
        server: String,
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl ServerSpec {
    /// The server key within the plugin's `mcpServers` map.
    #[must_use]
    pub fn server(&self) -> &str {
        match self {
            Self::Stdio { server, .. } | Self::StreamableHttp { server, .. } => server,
        }
    }
}

/// The parsed document: schema-validated top level, servers not yet
/// individually validated.
#[derive(Debug, Clone, PartialEq)]
pub struct McpDocument {
    servers: BTreeMap<String, Value>,
}

/// Per-server validation outcome (the failure boundary of REQ-AP-004).
#[derive(Debug, Clone, PartialEq)]
pub enum ServerValidation {
    Valid(ServerSpec),
    Skipped(ServerFailure),
}

/// Why one server entry was skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerFailure {
    pub server: String,
    pub reason: String,
}

impl McpDocument {
    /// Parse `mcp.json` and check its declared specification version against
    /// the manifest's (they must match exactly, REQ-AP-012).
    ///
    /// # Errors
    /// Returns a document-level failure reason for invalid JSON, a missing
    /// or non-object `mcpServers`, or a `$schema` that is missing,
    /// unrecognized, or different from the manifest's declared version.
    pub fn parse(content: &str, manifest_schema: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(content)
            .map_err(|e| format!("mcp.json is not valid JSON: {e}"))?;
        let Some(object) = root.as_object() else {
            return Err("mcp.json root is not a JSON object".to_string());
        };

        let Some(schema) = object.get("$schema").and_then(Value::as_str) else {
            return Err("mcp.json missing required $schema".to_string());
        };
        if schema != mcp_schema_for(manifest_schema) {
            return Err(format!(
                "mcp.json $schema ({schema}) does not match the manifest's \
                 specification version ({})",
                mcp_schema_for(manifest_schema)
            ));
        }

        let Some(servers) = object.get("mcpServers").and_then(Value::as_object) else {
            return Err("mcp.json missing required mcpServers object".to_string());
        };

        Ok(Self {
            servers: servers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    /// Validate each server independently (REQ-AP-004): one invalid entry
    /// skips that entry, never its siblings.
    #[must_use]
    pub fn validate_servers(&self) -> Vec<(String, ServerValidation)> {
        self.servers
            .iter()
            .map(|(name, entry)| (name.clone(), validate_server(name, entry)))
            .collect()
    }
}

/// Map a manifest schema identifier to its paired MCP schema identifier.
/// Both must declare the same specification version (REQ-AP-012).
fn mcp_schema_for(manifest_schema: &str) -> String {
    manifest_schema.replace("plugin.schema.json", "mcp.schema.json")
}

/// Validate one `mcpServers` entry against the closed variants.
fn validate_server(name: &str, entry: &Value) -> ServerValidation {
    let Some(fields) = entry.as_object() else {
        return skip(name, "server entry is not an object");
    };

    let transport = fields.get("type").and_then(Value::as_str);
    match transport {
        Some("stdio") => validate_stdio(name, fields),
        Some("streamable-http") => validate_http(name, fields),
        Some("sse") => skip(name, "deprecated sse transport is unsupported; skipped"),
        Some(other) => skip(name, &format!("unknown transport type: {other}")),
        None => skip(name, "server entry missing required type field"),
    }
}

fn validate_stdio(name: &str, fields: &serde_json::Map<String, Value>) -> ServerValidation {
    let Some(command) = fields.get("command").and_then(Value::as_str) else {
        return skip(name, "stdio server missing required command");
    };
    // One executable token: bare name or plugin-relative `./` path.
    // Shell command lines are structurally excluded (no spaces survive).
    if command.contains(char::is_whitespace) {
        return skip(name, "stdio command must be one executable token");
    }
    if command.contains('/') && !command.starts_with("./") {
        return skip(name, "stdio command must be bare or plugin-relative ./");
    }

    let Some(args) = string_array(fields, "args") else {
        return skip(name, "stdio args must be an array of strings");
    };

    let Some(env) = string_map(fields, "env") else {
        return skip(name, "stdio env must be an object with string values");
    };
    // Configured env may not declare the client-owned variables (REQ-AP-006).
    if env.contains_key("PLUGIN_ROOT") || env.contains_key("PLUGIN_DATA") {
        return skip(
            name,
            "stdio env must not declare PLUGIN_ROOT or PLUGIN_DATA",
        );
    }

    let cwd = match fields.get("cwd") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return skip(name, "stdio cwd must be a string"),
    };
    if let Some(cwd) = &cwd {
        if !cwd_is_wellformed(cwd) {
            return skip(
                name,
                "stdio cwd must be ./-relative or ${PLUGIN_ROOT}/${PLUGIN_DATA}-anchored",
            );
        }
    }

    // Unknown fields invalidate this server (closed variant).
    let known = ["type", "command", "args", "env", "cwd"];
    if let Some(unknown) = fields.keys().find(|k| !known.contains(&k.as_str())) {
        return skip(name, &format!("stdio server has unknown field: {unknown}"));
    }

    ServerValidation::Valid(ServerSpec::Stdio {
        server: name.to_string(),
        command: command.to_string(),
        args,
        env,
        cwd,
    })
}

fn validate_http(name: &str, fields: &serde_json::Map<String, Value>) -> ServerValidation {
    let Some(url) = fields.get("url").and_then(Value::as_str) else {
        return skip(name, "streamable-http server missing required url");
    };
    if !http_url_is_allowed(url) {
        return skip(
            name,
            "streamable-http url must be absolute http(s) with no userinfo or fragment; \
             non-loopback hosts require https",
        );
    }

    let headers = match fields.get("headers") {
        None => BTreeMap::new(),
        Some(_) => match string_map(fields, "headers") {
            Some(headers) => headers,
            None => {
                return skip(
                    name,
                    "streamable-http headers must be an object with string values",
                );
            }
        },
    };
    // Duplicate header names differing only in case invalidate the entry;
    // BTreeMap keys are case-sensitive so compare lowercased.
    let mut seen_lower: Vec<String> = Vec::new();
    for header in headers.keys() {
        let lower = header.to_ascii_lowercase();
        if seen_lower.contains(&lower) {
            return skip(name, "duplicate header names differing only in case");
        }
        seen_lower.push(lower);
    }

    let known = ["type", "url", "headers"];
    if let Some(unknown) = fields.keys().find(|k| !known.contains(&k.as_str())) {
        return skip(
            name,
            &format!("streamable-http server has unknown field: {unknown}"),
        );
    }

    ServerValidation::Valid(ServerSpec::StreamableHttp {
        server: name.to_string(),
        url: url.to_string(),
        headers,
    })
}

/// Absolute HTTP(S) URL, no userinfo, no fragment, and HTTPS unless the
/// host is exactly `localhost` or a loopback IP literal (REQ-AP-004).
fn http_url_is_allowed(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    if url.starts_with("http://") && !loopback_host(rest) {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return false;
    }
    if authority.contains('@') || authority.contains('#') || url.contains('#') {
        return false;
    }
    true
}

fn loopback_host(authority_and_path: &str) -> bool {
    let host = authority_and_path
        .split('/')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// `cwd` well-formedness before expansion (REQ-AP-006): `./`-relative or
/// anchored on `${PLUGIN_ROOT}` / `${PLUGIN_DATA}`.
fn cwd_is_wellformed(cwd: &str) -> bool {
    if cwd.starts_with("./") {
        return true;
    }
    if cwd == "${PLUGIN_ROOT}" || cwd.starts_with("${PLUGIN_ROOT}/") {
        return true;
    }
    if cwd == "${PLUGIN_DATA}" || cwd.starts_with("${PLUGIN_DATA}/") {
        return true;
    }
    false
}

fn string_array(fields: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match fields.get(key) {
        None => Some(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(item.as_str()?.to_string());
            }
            Some(out)
        }
        Some(_) => None,
    }
}

fn string_map(
    fields: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<BTreeMap<String, String>> {
    match fields.get(key) {
        None => Some(BTreeMap::new()),
        Some(Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (field, value) in map {
                out.insert(field.clone(), value.as_str()?.to_string());
            }
            Some(out)
        }
        Some(_) => None,
    }
}

fn skip(name: &str, reason: &str) -> ServerValidation {
    ServerValidation::Skipped(ServerFailure {
        server: name.to_string(),
        reason: reason.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::MANIFEST_SCHEMA_URL;

    fn doc(content: &str) -> McpDocument {
        McpDocument::parse(content, MANIFEST_SCHEMA_URL).unwrap()
    }

    fn valid_servers(content: &str) -> Vec<ServerSpec> {
        doc(content)
            .validate_servers()
            .into_iter()
            .filter_map(|(_, v)| match v {
                ServerValidation::Valid(spec) => Some(spec),
                ServerValidation::Skipped(_) => None,
            })
            .collect()
    }

    fn skipped_servers(content: &str) -> Vec<String> {
        doc(content)
            .validate_servers()
            .into_iter()
            .filter_map(|(_, v)| match v {
                ServerValidation::Skipped(f) => Some(f.server),
                ServerValidation::Valid(_) => None,
            })
            .collect()
    }

    #[test]
    fn parses_the_specification_complete_example() {
        let servers = valid_servers(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json",
                "mcpServers": {
                    "local-validator": {
                        "type": "stdio",
                        "command": "./bin/validator",
                        "args": ["--data", "${PLUGIN_DATA}/validator"],
                        "env": {"CONFIG": "${PLUGIN_ROOT}/config.json"},
                        "cwd": "${PLUGIN_ROOT}"
                    },
                    "deployment-api": {
                        "type": "streamable-http",
                        "url": "https://deploy.example.com/mcp",
                        "headers": {"X-Tenant": "public-tenant"}
                    },
                    "legacy-events": {
                        "type": "sse",
                        "url": "https://legacy.example.com/sse"
                    }
                }
            }"#,
        );
        assert_eq!(servers.len(), 2, "sse transport is skipped");
        let validator = servers
            .iter()
            .find(|s| s.server() == "local-validator")
            .expect("local-validator loads");
        assert!(
            matches!(validator, ServerSpec::Stdio { command, .. } if command == "./bin/validator")
        );
        let api = servers
            .iter()
            .find(|s| s.server() == "deployment-api")
            .expect("deployment-api loads");
        assert!(
            matches!(api, ServerSpec::StreamableHttp { url, .. } if url == "https://deploy.example.com/mcp")
        );
    }

    #[test]
    fn empty_mcp_servers_is_valid() {
        let servers = valid_servers(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": {}}"#,
        );
        assert!(servers.is_empty());
    }

    #[test]
    fn document_level_failures_disable_whole_component() {
        for bad in [
            "not json",
            "[1, 2]",
            r#"{"mcpServers": {}}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json"}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": []}"#,
        ] {
            assert!(
                McpDocument::parse(bad, MANIFEST_SCHEMA_URL).is_err(),
                "should reject document: {bad}"
            );
        }
    }

    #[test]
    fn version_mismatch_with_manifest_disables_mcp() {
        let err = McpDocument::parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": {}}"#,
            "https://agent-plugins.org/schemas/1.2.0/plugin.schema.json",
        )
        .unwrap_err();
        assert!(err.contains("does not match"));
    }

    #[test]
    fn one_bad_server_skips_only_itself() {
        let skipped = skipped_servers(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json",
                "mcpServers": {
                    "good": {"type": "streamable-http", "url": "https://example.com/mcp"},
                    "bad-type": {"type": "weird"},
                    "no-command": {"type": "stdio"},
                    "shell-line": {"type": "stdio", "command": "npx --yes foo"},
                    "claims-plugin-env": {
                        "type": "stdio",
                        "command": "npx",
                        "env": {"PLUGIN_ROOT": "/elsewhere"}
                    },
                    "absolute-cwd": {
                        "type": "stdio",
                        "command": "npx",
                        "cwd": "/etc"
                    },
                    "unknown-field": {
                        "type": "streamable-http",
                        "url": "https://example.com/mcp",
                        "timeout": 5
                    }
                }
            }"#,
        );
        for expected in [
            "bad-type",
            "no-command",
            "shell-line",
            "claims-plugin-env",
            "absolute-cwd",
            "unknown-field",
        ] {
            assert!(
                skipped.contains(&expected.to_string()),
                "should skip {expected}"
            );
        }
        let servers = valid_servers(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": {"good": {"type": "streamable-http", "url": "https://example.com/mcp"}}}"#,
        );
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn http_urls_follow_the_loopback_https_rule() {
        let skipped = skipped_servers(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json",
                "mcpServers": {
                    "loopback": {"type": "streamable-http", "url": "http://localhost:9700/mcp"},
                    "loopback-ip": {"type": "streamable-http", "url": "http://127.0.0.1:9700/mcp"},
                    "remote-http": {"type": "streamable-http", "url": "http://deploy.example.com/mcp"},
                    "userinfo": {"type": "streamable-http", "url": "https://user:pass@example.com/mcp"},
                    "fragment": {"type": "streamable-http", "url": "https://example.com/mcp#frag"},
                    "relative": {"type": "streamable-http", "url": "/mcp"}
                }
            }"#,
        );
        for expected in ["remote-http", "userinfo", "fragment", "relative"] {
            assert!(
                skipped.contains(&expected.to_string()),
                "should skip {expected}"
            );
        }
        assert_eq!(skipped.len(), 4, "loopback and loopback-ip are allowed");
    }

    #[test]
    fn case_conflicting_headers_are_rejected() {
        let skipped = skipped_servers(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json",
                "mcpServers": {
                    "dup-case": {
                        "type": "streamable-http",
                        "url": "https://example.com/mcp",
                        "headers": {"X-Tenant": "a", "x-tenant": "b"}
                    }
                }
            }"#,
        );
        assert_eq!(skipped, vec!["dup-case".to_string()]);
    }
}
