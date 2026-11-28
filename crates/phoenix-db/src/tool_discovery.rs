use std::collections::{BTreeMap, BTreeSet};

use phoenix_core::domain::{
    db_schema::{Message, MessageContent, ToolContent},
    llm_types::{ContentBlock, ToolDefinition},
    tool_availability::{ToolAvailability, ToolPolicyMessage},
    tool_discovery::{
        native_client_search_call, search_tools, tool_search_definition, tool_search_result_text,
        ToolDiscoveryMode, ToolDiscoveryProtocol, ToolDiscoveryResult, ToolSearchQuery,
        TOOL_SEARCH_NAME,
    },
};
use sqlx::{Row, Sqlite, Transaction};

use crate::{Database, DbError, DbResult};

type DiscoveryTransaction<'a> = Transaction<'a, Sqlite>;

fn invalid(message: &str) -> DbError {
    DbError::Serialization(message.into())
}

fn same_definition(left: &ToolDefinition, right: &ToolDefinition) -> bool {
    left.name == right.name
        && left.description == right.description
        && left.input_schema == right.input_schema
        && left.defer_loading == right.defer_loading
}

pub(crate) async fn catalog_tx(
    tx: &mut DiscoveryTransaction<'_>,
    conversation_id: &str,
) -> DbResult<Vec<ToolDefinition>> {
    sqlx::query("SELECT d.name,d.description,d.input_schema,d.defer_loading FROM conversation_tool_definitions d JOIN conversation_callable_tools c USING(conversation_id,name) WHERE d.conversation_id=? AND d.name<>? ORDER BY d.name")
        .bind(conversation_id).bind(TOOL_SEARCH_NAME).fetch_all(&mut **tx).await?
        .iter().map(crate::tool_availability::definition).collect()
}

async fn protocol_tx(
    tx: &mut DiscoveryTransaction<'_>,
    assistant: &Message,
    call_id: &str,
) -> DbResult<ToolDiscoveryProtocol> {
    let row = sqlx::query("SELECT response_id,public_content,api_format FROM active_responses_replay_sets WHERE conversation_id=? AND owner_message_id=?")
        .bind(&assistant.conversation_id).bind(&assistant.message_id)
        .fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(ToolDiscoveryProtocol::Function);
    };
    let public: Vec<ContentBlock> =
        serde_json::from_str(&row.try_get::<String, _>("public_content")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Err(invalid("tool discovery owner is not an assistant"));
    };
    if &public != blocks {
        return Err(invalid("tool discovery private owner witness differs"));
    }
    if row.try_get::<String, _>("api_format")? != "responses" {
        return Ok(ToolDiscoveryProtocol::Function);
    }
    let payloads: Vec<String> = sqlx::query_scalar("SELECT payload FROM active_responses_replay_items WHERE conversation_id=? AND response_id=? ORDER BY ordinal")
        .bind(&assistant.conversation_id).bind(row.try_get::<String, _>("response_id")?)
        .fetch_all(&mut **tx).await?;
    let mut native = false;
    let mut ordinary = false;
    for payload in payloads {
        let item: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        if item.get("call_id").and_then(serde_json::Value::as_str) != Some(call_id) {
            continue;
        }
        if native_client_search_call(&item)
            .map_err(DbError::Serialization)?
            .is_some()
        {
            native = true;
        } else if item.get("type").and_then(serde_json::Value::as_str) == Some("function_call") {
            ordinary = true;
        }
    }
    if native && ordinary {
        return Err(invalid("ambiguous tool discovery protocol"));
    }
    Ok(if native {
        ToolDiscoveryProtocol::OpenAiClient
    } else {
        ToolDiscoveryProtocol::Function
    })
}

fn protocol_name(protocol: ToolDiscoveryProtocol) -> &'static str {
    match protocol {
        ToolDiscoveryProtocol::Function => "function",
        ToolDiscoveryProtocol::OpenAiClient => "openai_client",
    }
}

async fn load_result_tx(
    tx: &mut DiscoveryTransaction<'_>,
    result_message_id: &str,
) -> DbResult<Option<ToolDiscoveryResult>> {
    let row = sqlx::query("SELECT r.owner_message_id,r.tool_call_id,r.protocol,m.content FROM tool_discovery_results r JOIN messages m ON m.message_id=r.result_message_id WHERE r.result_message_id=?")
        .bind(result_message_id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(None) };
    let children = sqlx::query("SELECT ordinal,name,description,input_schema,defer_loading FROM tool_discovery_result_tools WHERE result_message_id=? ORDER BY ordinal")
        .bind(result_message_id).fetch_all(&mut **tx).await?;
    let mut tools = Vec::with_capacity(children.len());
    for (ordinal, child) in children.iter().enumerate() {
        if child.try_get::<i64, _>("ordinal")?
            != i64::try_from(ordinal).map_err(|error| DbError::Serialization(error.to_string()))?
        {
            return Err(invalid("tool discovery ordinals are not contiguous"));
        }
        tools.push(crate::tool_availability::definition(child)?);
    }
    let content: ToolContent = serde_json::from_str(&row.try_get::<String, _>("content")?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let call_id: String = row.try_get("tool_call_id")?;
    if content.tool_use_id != call_id
        || (content.is_error && !tools.is_empty())
        || (!content.is_error && tool_search_result_text(&tools) != content.content())
    {
        return Err(invalid("tool discovery result witness differs"));
    }
    let protocol = match row.try_get::<String, _>("protocol")?.as_str() {
        "function" => ToolDiscoveryProtocol::Function,
        "openai_client" => ToolDiscoveryProtocol::OpenAiClient,
        _ => return Err(invalid("unknown tool discovery protocol")),
    };
    Ok(Some(ToolDiscoveryResult {
        owner_message_id: row.try_get("owner_message_id")?,
        tool_call_id: call_id,
        result_message_id: result_message_id.into(),
        protocol,
        tools,
    }))
}

pub(crate) async fn record_round_tx(
    tx: &mut DiscoveryTransaction<'_>,
    assistant: &Message,
    results: &[Message],
    replayed: bool,
) -> DbResult<()> {
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Ok(());
    };
    for result in results {
        let MessageContent::Tool(content) = &result.content else {
            continue;
        };
        let Some(input) = blocks.iter().find_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                (id == &content.tool_use_id && name == TOOL_SEARCH_NAME).then_some(input)
            } else {
                None
            }
        }) else {
            continue;
        };
        if let Some(stored) = load_result_tx(tx, &result.message_id).await? {
            if stored.owner_message_id != assistant.message_id
                || stored.tool_call_id != content.tool_use_id
                || (!content.is_error
                    && tool_search_result_text(&stored.tools) != content.content())
            {
                return Err(invalid("immutable tool discovery result differs"));
            }
            continue;
        }
        if replayed {
            continue;
        }
        let tools = if content.is_error {
            Vec::new()
        } else {
            let schema: Option<String> = sqlx::query_scalar("SELECT d.input_schema FROM conversation_tool_definitions d JOIN conversation_callable_tools c USING(conversation_id,name) JOIN conversation_advertised_tools a USING(conversation_id,name) WHERE d.conversation_id=? AND d.name=?")
                .bind(&assistant.conversation_id).bind(TOOL_SEARCH_NAME).fetch_optional(&mut **tx).await?;
            let schema: serde_json::Value =
                serde_json::from_str(&schema.ok_or_else(|| {
                    invalid("tool discovery has no admitted request declaration")
                })?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            if schema != tool_search_definition().input_schema {
                return Err(invalid("tool discovery request schema differs"));
            }
            let query = ToolSearchQuery::parse_input(input).map_err(DbError::Serialization)?;
            let tools = search_tools(&catalog_tx(tx, &assistant.conversation_id).await?, &query);
            if tool_search_result_text(&tools) != content.content() {
                return Err(invalid("tool discovery output differs from frozen catalog"));
            }
            tools
        };
        let protocol = protocol_tx(tx, assistant, &content.tool_use_id).await?;
        sqlx::query("INSERT INTO tool_discovery_results(conversation_id,owner_message_id,tool_call_id,result_message_id,protocol) VALUES (?,?,?,?,?)")
            .bind(&assistant.conversation_id).bind(&assistant.message_id).bind(&content.tool_use_id).bind(&result.message_id).bind(protocol_name(protocol)).execute(&mut **tx).await?;
        for (ordinal, tool) in tools.iter().enumerate() {
            sqlx::query("INSERT INTO tool_discovery_result_tools(result_message_id,ordinal,name,description,input_schema,defer_loading) VALUES (?,?,?,?,?,?)")
                .bind(&result.message_id).bind(i64::try_from(ordinal).map_err(|error| DbError::Serialization(error.to_string()))?)
                .bind(&tool.name).bind(&tool.description).bind(serde_json::to_string(&tool.input_schema).map_err(|error| DbError::Serialization(error.to_string()))?).bind(tool.defer_loading).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

async fn visible_results_tx(
    tx: &mut DiscoveryTransaction<'_>,
    conversation_id: &str,
    visible: &BTreeMap<&str, usize>,
) -> DbResult<Vec<ToolDiscoveryResult>> {
    let rows = sqlx::query("SELECT r.result_message_id,r.owner_message_id FROM tool_discovery_results r JOIN messages m ON m.message_id=r.result_message_id WHERE r.conversation_id=? ORDER BY m.sequence_id,r.result_message_id")
        .bind(conversation_id).fetch_all(&mut **tx).await?;
    let mut results = Vec::new();
    for row in rows {
        let owner: String = row.try_get("owner_message_id")?;
        let result_id: String = row.try_get("result_message_id")?;
        if visible.contains_key(owner.as_str()) && visible.contains_key(result_id.as_str()) {
            results.push(
                load_result_tx(tx, &result_id)
                    .await?
                    .ok_or_else(|| invalid("tool discovery result disappeared"))?,
            );
        }
    }
    results.sort_by_key(|result| visible[result.result_message_id.as_str()]);
    Ok(results)
}

async fn historical_names_tx(
    tx: &mut DiscoveryTransaction<'_>,
    conversation_id: &str,
    visible: &BTreeMap<&str, usize>,
) -> DbResult<Vec<(usize, usize, String)>> {
    let rows = sqlx::query("SELECT message_id,content FROM messages WHERE conversation_id=? AND message_type='agent' ORDER BY sequence_id")
        .bind(conversation_id).fetch_all(&mut **tx).await?;
    let mut names = Vec::new();
    for row in rows {
        let id: String = row.try_get("message_id")?;
        let Some(position) = visible.get(id.as_str()) else {
            continue;
        };
        let blocks: Vec<ContentBlock> = serde_json::from_str(&row.try_get::<String, _>("content")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        names.extend(
            blocks
                .into_iter()
                .enumerate()
                .filter_map(|(ordinal, block)| {
                    if let ContentBlock::ToolUse { name, .. } = block {
                        Some((*position, ordinal, name))
                    } else {
                        None
                    }
                }),
        );
    }
    Ok(names)
}

fn projected_declarations(
    policy: &ToolAvailability,
    mode: ToolDiscoveryMode,
    results: &[ToolDiscoveryResult],
    visible: &BTreeMap<&str, usize>,
    historical: &[(usize, usize, String)],
) -> Vec<ToolDefinition> {
    let live = policy
        .declarations()
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect::<BTreeMap<_, _>>();
    let mut events = Vec::new();
    for result in results {
        if mode != ToolDiscoveryMode::Local
            && result.protocol != ToolDiscoveryProtocol::OpenAiClient
        {
            continue;
        }
        for (ordinal, tool) in result.tools.iter().enumerate() {
            if policy.is_callable(&tool.name)
                && live
                    .get(tool.name.as_str())
                    .is_some_and(|current| same_definition(tool, current))
            {
                events.push((
                    visible[result.result_message_id.as_str()],
                    ordinal,
                    tool.name.as_str(),
                ));
            }
        }
    }
    for (position, ordinal, name) in historical {
        if live.contains_key(name.as_str()) {
            events.push((*position, *ordinal, name.as_str()));
        }
    }
    events.sort_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)));
    let mut declarations = policy
        .declarations()
        .iter()
        .filter(|tool| !tool.defer_loading)
        .cloned()
        .collect::<Vec<_>>();
    let mut seen = declarations
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<BTreeSet<_>>();
    for (_, _, name) in events {
        if seen.insert(name.to_owned()) {
            declarations.push((*live[name]).clone());
        }
    }
    declarations
}

pub(crate) async fn project_tx(
    tx: &mut DiscoveryTransaction<'_>,
    conversation_id: &str,
    mode: ToolDiscoveryMode,
    policy: ToolAvailability,
    visible_messages: &[ToolPolicyMessage],
) -> DbResult<ToolAvailability> {
    let mut visible = BTreeMap::new();
    for (position, message) in visible_messages.iter().enumerate() {
        if let Some(id) = message.source_message_id.as_deref() {
            visible.entry(id).or_insert(position);
        }
    }
    let results = visible_results_tx(tx, conversation_id, &visible).await?;
    let mut snapshot = if mode.uses_local_search() {
        let historical = if mode == ToolDiscoveryMode::Local {
            historical_names_tx(tx, conversation_id, &visible).await?
        } else {
            Vec::new()
        };
        let permitted = policy.callable_names();
        let declarations = projected_declarations(&policy, mode, &results, &visible, &historical);
        let names = declarations
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();
        let callable = permitted
            .iter()
            .filter(|name| names.contains(name.as_str()))
            .cloned()
            .collect();
        let mut projected =
            ToolAvailability::new(declarations, callable).map_err(DbError::Serialization)?;
        if let Some(id) = policy.continuation_id() {
            projected = projected
                .with_continuation_id(id.to_owned())
                .map_err(DbError::Serialization)?;
        }
        projected
    } else {
        policy
    };
    snapshot = snapshot
        .with_discovery(mode, results)
        .map_err(DbError::Serialization)?;
    sqlx::query("DELETE FROM conversation_advertised_tools WHERE conversation_id=?")
        .bind(conversation_id)
        .execute(&mut **tx)
        .await?;
    for definition in snapshot.declarations() {
        sqlx::query("INSERT INTO conversation_advertised_tools(conversation_id,name) VALUES (?,?)")
            .bind(conversation_id)
            .bind(&definition.name)
            .execute(&mut **tx)
            .await?;
    }
    Ok(snapshot)
}

impl Database {
    /// # Errors
    /// Returns failures reading the frozen full permitted catalog.
    pub async fn load_tool_search_catalog(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<ToolDefinition>> {
        let mut tx = self.pool().begin().await?;
        let tools = catalog_tx(&mut tx, conversation_id).await?;
        tx.commit().await?;
        Ok(tools)
    }
}

#[cfg(test)]
mod tests;
