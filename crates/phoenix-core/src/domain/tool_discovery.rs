//! Tool discovery selects authentic schemas without granting execution authority.

use super::llm_types::ToolDefinition;

pub const TOOL_SEARCH_NAME: &str = "tool_search";
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 20;
const MAX_QUERY_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolDiscoveryMode {
    #[default]
    Eager,
    Anthropic,
    OpenAiHosted,
    OpenAiClient,
    Local,
}

impl ToolDiscoveryMode {
    #[must_use]
    pub const fn uses_local_search(self) -> bool {
        match self {
            Self::OpenAiClient | Self::Local => true,
            Self::Eager | Self::Anthropic | Self::OpenAiHosted => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDiscoveryProtocol {
    Function,
    OpenAiClient,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeToolSearchCall {
    pub call_id: String,
    pub arguments: serde_json::Value,
}

/// # Errors
/// Rejects malformed native client calls without including provider content.
pub fn native_client_search_call(
    item: &serde_json::Value,
) -> Result<Option<NativeToolSearchCall>, String> {
    let arguments = match item.get("type").and_then(serde_json::Value::as_str) {
        Some("tool_search_call")
            if item.get("execution").and_then(serde_json::Value::as_str) == Some("client") =>
        {
            item.get("arguments").cloned()
        }
        Some("function_call")
            if item.get("namespace").and_then(serde_json::Value::as_str) == Some("tool_search")
                && item.get("name").and_then(serde_json::Value::as_str)
                    == Some("tool_search_tool") =>
        {
            item.get("arguments")
                .and_then(serde_json::Value::as_str)
                .and_then(|raw| serde_json::from_str(raw).ok())
        }
        _ => return Ok(None),
    };
    let arguments = arguments
        .filter(serde_json::Value::is_object)
        .ok_or("Native tool search arguments must be an object")?;
    let call_id = item
        .get("call_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("Native tool search call has no identity")?;
    Ok(Some(NativeToolSearchCall {
        call_id: call_id.to_owned(),
        arguments,
    }))
}

#[derive(Clone, Debug)]
pub struct ToolDiscoveryResult {
    pub owner_message_id: String,
    pub tool_call_id: String,
    pub result_message_id: String,
    pub protocol: ToolDiscoveryProtocol,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Clone, Debug)]
pub struct ToolSearchQuery {
    query: String,
    limit: usize,
}

impl ToolSearchQuery {
    /// # Errors
    /// Rejects unknown fields, empty queries, and unbounded input or result limits.
    pub fn parse_input(input: &serde_json::Value) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            query: String,
            limit: Option<usize>,
        }
        let input: Input = serde_json::from_value(input.clone()).map_err(|_| {
            "tool_search requires query text and an optional integer limit".to_owned()
        })?;
        let query = input.query.trim();
        if query.is_empty()
            || query.len() > MAX_QUERY_BYTES
            || !query.chars().any(char::is_alphanumeric)
        {
            return Err(format!(
                "tool_search query must contain searchable text within {MAX_QUERY_BYTES} bytes"
            ));
        }
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(format!(
                "tool_search limit must be between 1 and {MAX_LIMIT}"
            ));
        }
        Ok(Self {
            query: query.to_owned(),
            limit,
        })
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

#[must_use]
pub fn tool_search_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_SEARCH_NAME.into(),
        description: "Find optional tools by task, service, or tool name. Search before using a tool that is not yet loaded. Matching tools become available after this result; discovery does not change their permissions.".into(),
        input_schema: serde_json::json!({
            "type":"object", "properties":{
                "query":{"type":"string","description":"Task, service, or tool name to search for.","minLength":1,"maxLength":MAX_QUERY_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_LIMIT,"default":DEFAULT_LIMIT}
            },"required":["query"],"additionalProperties":false
        }),
        defer_loading: false,
    }
}

#[must_use]
pub fn search_tools(catalog: &[ToolDefinition], query: &ToolSearchQuery) -> Vec<ToolDefinition> {
    let normalized = query.query.to_lowercase();
    let terms = normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    let mut ranked = catalog
        .iter()
        .filter(|tool| tool.defer_loading && tool.name != TOOL_SEARCH_NAME)
        .filter_map(|tool| {
            let name = tool.name.to_lowercase();
            let description = tool.description.to_lowercase();
            let schema = tool.input_schema.to_string().to_lowercase();
            let mut score = if name == normalized { 10_000 } else { 0 };
            for term in &terms {
                score += 100 * i32::from(name.contains(term));
                score += 10 * i32::from(description.contains(term));
                score += i32::from(schema.contains(term));
            }
            (score > 0).then_some((score, tool))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(score_a, a), (score_b, b)| {
        score_b.cmp(score_a).then_with(|| a.name.cmp(&b.name))
    });
    ranked
        .into_iter()
        .take(query.limit)
        .map(|(_, tool)| tool.clone())
        .collect()
}

#[must_use]
pub fn tool_search_result_text(tools: &[ToolDefinition]) -> String {
    serde_json::json!({"tools":tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_search_call_formats_have_one_typed_projection() {
        let canonical = serde_json::json!({"type":"tool_search_call","execution":"client","call_id":"call","arguments":{"query":"issues"}});
        let namespaced = serde_json::json!({"type":"function_call","namespace":"tool_search","name":"tool_search_tool","call_id":"call","arguments":"{\"query\":\"issues\"}"});
        let expected = Some(NativeToolSearchCall {
            call_id: "call".into(),
            arguments: serde_json::json!({"query":"issues"}),
        });
        assert_eq!(native_client_search_call(&canonical).unwrap(), expected);
        assert_eq!(native_client_search_call(&namespaced).unwrap(), expected);
        for key in ["namespace", "name", "type"] {
            let mut ordinary = namespaced.clone();
            ordinary[key] = serde_json::json!("other");
            assert!(native_client_search_call(&ordinary).unwrap().is_none());
        }
        for (key, value) in [
            ("arguments", serde_json::json!("invalid JSON")),
            ("arguments", serde_json::json!("[]")),
            ("arguments", serde_json::json!({"query":"issues"})),
            ("call_id", serde_json::json!("")),
        ] {
            let mut malformed = namespaced.clone();
            malformed[key] = value;
            assert!(native_client_search_call(&malformed).is_err());
        }
    }

    fn tool(name: &str, description: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            input_schema: schema,
            defer_loading: true,
        }
    }

    #[test]
    fn search_rejects_invalid_or_unbounded_queries() {
        for input in [
            serde_json::json!({}),
            serde_json::json!({"query":" "}),
            serde_json::json!({"query":"***"}),
            serde_json::json!({"query":"x","limit":0}),
            serde_json::json!({"query":"x","limit":21}),
            serde_json::json!({"query":"x","limit":-1}),
            serde_json::json!({"query":"x","extra":true}),
            serde_json::json!({"query":"x".repeat(513)}),
        ] {
            assert!(ToolSearchQuery::parse_input(&input).is_err());
        }
    }

    #[test]
    fn search_ranks_names_before_description_and_schema_with_stable_ties() {
        let catalog = vec![
            tool("other", "Find issues", serde_json::json!({})),
            tool("mcp__github__issues", "", serde_json::json!({})),
            tool(
                "schema",
                "",
                serde_json::json!({"properties":{"issue":{"type":"string"}}}),
            ),
            tool("a_issue", "", serde_json::json!({})),
        ];
        let query =
            ToolSearchQuery::parse_input(&serde_json::json!({"query":"ISSUE","limit":3})).unwrap();
        let found = search_tools(&catalog, &query);
        assert_eq!(
            found
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a_issue", "mcp__github__issues", "other"]
        );
        assert_eq!(
            tool_search_result_text(&found),
            r#"{"tools":["a_issue","mcp__github__issues","other"]}"#
        );
        assert!(search_tools(
            &catalog,
            &ToolSearchQuery::parse_input(&serde_json::json!({"query":"missing"})).unwrap()
        )
        .is_empty());
    }

    #[test]
    fn search_does_not_return_eager_or_search_itself() {
        let mut eager = tool("eager", "find issue", serde_json::json!({}));
        eager.defer_loading = false;
        let catalog = vec![eager, tool_search_definition()];
        let query = ToolSearchQuery::parse_input(&serde_json::json!({"query":"find"})).unwrap();
        assert!(search_tools(&catalog, &query).is_empty());
    }
}
