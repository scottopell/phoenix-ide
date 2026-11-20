use serde::{Deserialize, Serialize};
use serde_json::Value;

use phoenix_core::domain::llm_types::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, ToolDefinition,
};
use phoenix_core::domain::tool_discovery::{
    ToolDiscoveryMode, ToolDiscoveryProtocol, TOOL_SEARCH_NAME,
};

use super::{
    InputMessageContent, InputMessagePart, InputMessageRole, LlmError, ModelSpec,
    NativeToolSearchExecution, ResponsesApiInputItem, ResponsesApiTool,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientExecution {
    Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletedStatus {
    Completed,
}

pub(super) fn native_mode(
    spec: &ModelSpec,
    codex: bool,
    endpoint: Option<&str>,
) -> ToolDiscoveryMode {
    if spec.source != crate::models::ModelSource::BuiltIn
        || spec.backend != crate::models::ModelBackend::OpenAIResponses
        || !super::supports_modern_responses(spec)
    {
        return ToolDiscoveryMode::Eager;
    }
    if codex {
        ToolDiscoveryMode::OpenAiClient
    } else if super::is_official_responses_route(endpoint)
        || endpoint == Some("https://ai-gateway.us1.ddbuild.io/v1/responses")
    {
        ToolDiscoveryMode::OpenAiHosted
    } else {
        ToolDiscoveryMode::Eager
    }
}

pub(super) fn validate_mode(
    spec: &ModelSpec,
    request: &LlmRequest,
    codex: bool,
    endpoint: Option<&str>,
) -> Result<(), LlmError> {
    let requested = request.tool_availability.discovery_mode();
    if matches!(
        requested,
        ToolDiscoveryMode::OpenAiHosted | ToolDiscoveryMode::OpenAiClient
    ) && requested != native_mode(spec, codex, endpoint)
    {
        return Err(LlmError::invalid_request(
            "Native tool discovery is unavailable on this model route",
        ));
    }
    Ok(())
}

fn function(tool: &ToolDefinition, defer: bool) -> ResponsesApiTool {
    ResponsesApiTool::Function {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
        defer_loading: (defer && tool.defer_loading).then_some(true),
    }
}

pub(super) fn request_tools(request: &LlmRequest) -> Option<Vec<ResponsesApiTool>> {
    let availability = &request.tool_availability;
    let mode = availability.discovery_mode();
    let mut tools: Vec<_> = availability
        .declarations()
        .iter()
        .filter(|tool| match mode {
            ToolDiscoveryMode::OpenAiHosted => {
                tool.name != TOOL_SEARCH_NAME && availability.is_callable(&tool.name)
            }
            ToolDiscoveryMode::OpenAiClient => tool.name != TOOL_SEARCH_NAME && !tool.defer_loading,
            ToolDiscoveryMode::Eager | ToolDiscoveryMode::Anthropic | ToolDiscoveryMode::Local => {
                true
            }
        })
        .map(|tool| function(tool, mode == ToolDiscoveryMode::OpenAiHosted))
        .collect();
    match mode {
        ToolDiscoveryMode::OpenAiHosted
            if availability
                .declarations()
                .iter()
                .any(|tool| tool.defer_loading && availability.is_callable(&tool.name)) =>
        {
            tools.push(ResponsesApiTool::Search {
                execution: NativeToolSearchExecution::Server,
            });
        }
        ToolDiscoveryMode::OpenAiClient if availability.is_callable(TOOL_SEARCH_NAME) => {
            let definition = phoenix_core::domain::tool_discovery::tool_search_definition();
            tools.push(ResponsesApiTool::Search {
                execution: NativeToolSearchExecution::Client {
                    description: definition.description,
                    parameters: definition.input_schema,
                },
            });
        }
        ToolDiscoveryMode::Eager
        | ToolDiscoveryMode::Anthropic
        | ToolDiscoveryMode::Local
        | ToolDiscoveryMode::OpenAiHosted
        | ToolDiscoveryMode::OpenAiClient => {}
    }
    (!tools.is_empty()).then_some(tools)
}

pub(super) fn translate_call(
    request: &LlmRequest,
    message: &LlmMessage,
    call_id: &str,
    name: &str,
    arguments: &Value,
) -> Option<ResponsesApiInputItem> {
    if request.tool_availability.discovery_mode() != ToolDiscoveryMode::OpenAiClient
        || name != TOOL_SEARCH_NAME
    {
        return None;
    }
    let owner = message.source_message_id.as_deref()?;
    request
        .tool_availability
        .discovery_results()
        .iter()
        .find(|result| {
            result.protocol == ToolDiscoveryProtocol::OpenAiClient
                && result.owner_message_id == owner
                && result.tool_call_id == call_id
        })?;
    Some(ResponsesApiInputItem::ToolSearchCall {
        call_id: call_id.into(),
        execution: ClientExecution::Client,
        arguments: arguments.clone(),
    })
}

pub(super) fn translate_result(
    request: &LlmRequest,
    message: &LlmMessage,
    call_id: &str,
    content: &str,
    is_error: bool,
) -> Option<Vec<ResponsesApiInputItem>> {
    if request.tool_availability.discovery_mode() != ToolDiscoveryMode::OpenAiClient {
        return None;
    }
    let result = request
        .tool_availability
        .discovery_result(message.source_message_id.as_deref()?, call_id)?;
    if result.protocol != ToolDiscoveryProtocol::OpenAiClient {
        return None;
    }
    let mut items = vec![ResponsesApiInputItem::ToolSearchOutput {
        call_id: call_id.into(),
        execution: ClientExecution::Client,
        status: CompletedStatus::Completed,
        tools: result
            .tools
            .iter()
            .map(|tool| function(tool, true))
            .collect(),
    }];
    if is_error {
        items.push(ResponsesApiInputItem::InputMessage {
            role: InputMessageRole::User,
            content: InputMessageContent::Parts(vec![InputMessagePart::InputText {
                text: format!("Tool search failed: {content}"),
                prompt_cache_breakpoint: None,
            }]),
        });
    }
    Some(items)
}

pub(super) fn validate_results(request: &LlmRequest) -> Result<(), LlmError> {
    if request.tool_availability.discovery_mode() != ToolDiscoveryMode::OpenAiClient {
        return Ok(());
    }
    for result in request
        .tool_availability
        .discovery_results()
        .iter()
        .filter(|result| result.protocol == ToolDiscoveryProtocol::OpenAiClient)
    {
        let owners: Vec<_> = request
            .messages
            .iter()
            .filter(|message| {
                message.source_message_id.as_deref() == Some(&result.owner_message_id)
            })
            .collect();
        let results: Vec<_> = request
            .messages
            .iter()
            .filter(|message| {
                message.source_message_id.as_deref() == Some(&result.result_message_id)
            })
            .collect();
        if owners.len()!=1 || owners[0].role != MessageRole::Assistant || results.len()!=1 || results[0].role != MessageRole::User
            || owners[0].content.iter().filter(|block| matches!(block, ContentBlock::ToolUse { id, name, input } if id == &result.tool_call_id && name == TOOL_SEARCH_NAME && input.is_object())).count()!=1
        { return Err(LlmError::invalid_request("Native tool discovery result has no authentic call/result owner")); }
        let matching: Vec<_> = results[0]
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    images,
                    ..
                } = block
                {
                    (tool_use_id == &result.tool_call_id).then_some((*is_error, images))
                } else {
                    None
                }
            })
            .collect();
        if matching.len() != 1
            || !matching[0].1.is_empty()
            || (matching[0].0 && !result.tools.is_empty())
        {
            return Err(LlmError::invalid_request(
                "Native tool discovery result is malformed",
            ));
        }
    }
    for set in request
        .output_replay
        .iter()
        .filter_map(phoenix_core::domain::responses_replay::ProviderResponseSet::as_responses)
    {
        for item in &set.output_items {
            if item.get("type").and_then(Value::as_str) != Some("tool_search_call")
                || item.get("execution").and_then(Value::as_str) != Some("client")
            {
                continue;
            }
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| LlmError::invalid_request("Native search call has no identity"))?;
            let records = request
                .tool_availability
                .discovery_results()
                .iter()
                .filter(|result| {
                    result.protocol == ToolDiscoveryProtocol::OpenAiClient
                        && result.owner_message_id == set.owner_message_id
                        && result.tool_call_id == call_id
                })
                .count();
            if records != 1 {
                return Err(LlmError::invalid_request(
                    "Native search result has no authenticated discovery record",
                ));
            }
        }
    }
    Ok(())
}

pub(super) enum SearchProjection {
    Hosted,
    ClientCall { call_id: String, arguments: Value },
}

pub(super) fn append_projection(
    item: &Value,
    content: &mut Vec<ContentBlock>,
) -> Result<bool, LlmError> {
    let Some(projection) = project_output(item)? else {
        return Ok(false);
    };
    match projection {
        SearchProjection::Hosted => {}
        SearchProjection::ClientCall { call_id, arguments } => {
            content.push(ContentBlock::ToolUse {
                id: call_id,
                name: TOOL_SEARCH_NAME.into(),
                input: arguments,
            });
        }
    }
    Ok(true)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SearchExecution {
    Server,
    Client,
}

#[derive(Deserialize)]
struct SearchCall {
    execution: SearchExecution,
    call_id: Option<String>,
    arguments: Value,
}

#[derive(Deserialize)]
struct SearchOutput {
    execution: SearchExecution,
    call_id: Option<String>,
    status: String,
    tools: Vec<Value>,
}

pub(super) fn project_output(item: &Value) -> Result<Option<SearchProjection>, LlmError> {
    let malformed = || LlmError::invalid_response("Malformed native Responses tool search item");
    match item.get("type").and_then(Value::as_str) {
        Some("tool_search_call") => {
            let call: SearchCall = serde_json::from_value(item.clone()).map_err(|_| malformed())?;
            if !call.arguments.is_object() {
                return Err(malformed());
            }
            match call.execution {
                SearchExecution::Server if call.call_id.is_none() => {
                    tracing::debug!(
                        native_tool_search = "server_call",
                        "retaining hosted search in private Responses replay"
                    );
                    Ok(Some(SearchProjection::Hosted))
                }
                SearchExecution::Client => {
                    let call_id = call
                        .call_id
                        .filter(|id| !id.is_empty())
                        .ok_or_else(malformed)?;
                    Ok(Some(SearchProjection::ClientCall {
                        call_id,
                        arguments: call.arguments,
                    }))
                }
                SearchExecution::Server => Err(malformed()),
            }
        }
        Some("tool_search_output") => {
            let output: SearchOutput =
                serde_json::from_value(item.clone()).map_err(|_| malformed())?;
            if !matches!(output.execution, SearchExecution::Server)
                || output.call_id.is_some()
                || output.status != "completed"
                || output.tools.iter().any(|tool| {
                    !tool.is_object() || tool.get("type").and_then(Value::as_str).is_none()
                })
            {
                return Err(malformed());
            }
            tracing::debug!(
                native_tool_search = "server_output",
                loaded_tools = output.tools.len(),
                "retaining hosted search in private Responses replay"
            );
            Ok(Some(SearchProjection::Hosted))
        }
        _ => Ok(None),
    }
}
