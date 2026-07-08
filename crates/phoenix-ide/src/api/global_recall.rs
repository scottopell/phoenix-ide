use super::AppState;
use crate::db::{ConvMode, Conversation, DbError, MessageType, RetrievalScope};
use crate::state_machine::ConvState;
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use phoenix_core::domain::message_text::index_text;
use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, PromptCacheKey, SystemContent,
    ToolDefinition,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::handlers::AppError;

const OPEN_WORK_LIMIT: usize = 80;
const RECENT_DAYS: i64 = 45;
const SEARCH_TOP_K: usize = 10;
const READ_PAGE_CHARS: usize = 7000;
const MAX_RECALL_TURNS: usize = 6;
const ANSWER_MAX_TOKENS: u32 = 3072;

#[derive(Debug, Serialize)]
pub struct GlobalOpenWorkResponse {
    pub generated_at: DateTime<Utc>,
    pub groups: Vec<GlobalOpenWorkProject>,
}

#[derive(Debug, Serialize)]
pub struct GlobalOpenWorkProject {
    pub project_id: Option<String>,
    pub project_name: String,
    pub canonical_path: Option<String>,
    pub items: Vec<GlobalOpenWorkItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalOpenWorkSource {
    Chain,
    Conversation,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalOpenWorkItem {
    pub id: String,
    pub source: GlobalOpenWorkSource,
    pub title: String,
    pub project_id: Option<String>,
    pub current_conversation_id: String,
    pub current_conversation_slug: Option<String>,
    pub root_conversation_id: String,
    pub root_conversation_slug: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub mode: String,
    pub state: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub task_status: Option<String>,
    pub branch_name: Option<String>,
    pub base_branch: Option<String>,
    pub worktree_path: Option<String>,
    pub member_count: usize,
    pub signals: Vec<String>,
    pub href: String,
    pub reference: String,
}

#[derive(Debug, Serialize)]
pub struct GlobalRecallSessionsResponse {
    pub sessions: Vec<GlobalRecallSession>,
}

#[derive(Debug, Serialize)]
pub struct GlobalRecallSessionResponse {
    pub session: GlobalRecallSession,
    pub messages: Vec<GlobalRecallMessage>,
}

#[derive(Debug, Serialize)]
pub struct GlobalRecallSession {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct GlobalRecallMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGlobalRecallSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateGlobalRecallSessionResponse {
    pub session: GlobalRecallSession,
}

#[derive(Debug, Deserialize)]
pub struct AskGlobalRecallRequest {
    pub question: String,
}

#[derive(Debug, Serialize)]
pub struct AskGlobalRecallResponse {
    pub user_message: GlobalRecallMessage,
    pub assistant_message: GlobalRecallMessage,
}

#[derive(Debug, Deserialize)]
pub struct ResolveGlobalReferenceRequest {
    pub reference: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveGlobalReferenceResponse {
    pub kind: String,
    pub id: String,
    pub href: Option<String>,
    pub title: Option<String>,
    pub summary: String,
}

pub async fn open_work(
    State(state): State<AppState>,
) -> Result<Json<GlobalOpenWorkResponse>, AppError> {
    Ok(Json(build_open_work(&state).await?))
}

pub async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<GlobalRecallSessionsResponse>, AppError> {
    let sessions = sqlx::query(
        "SELECT id, title, created_at, updated_at FROM global_recall_sessions ORDER BY updated_at DESC",
    )
    .try_map(parse_session_row)
    .fetch_all(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(GlobalRecallSessionsResponse { sessions }))
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateGlobalRecallSessionRequest>,
) -> Result<Json<CreateGlobalRecallSessionResponse>, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Global Recall")
        .to_string();
    sqlx::query(
        "INSERT INTO global_recall_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&id)
    .bind(&title)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CreateGlobalRecallSessionResponse {
        session: GlobalRecallSession {
            id,
            title,
            created_at: now,
            updated_at: now,
        },
    }))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GlobalRecallSessionResponse>, AppError> {
    let session = load_session(&state, &id).await?;
    let messages = load_session_messages(&state, &id).await?;
    Ok(Json(GlobalRecallSessionResponse { session, messages }))
}

pub async fn ask_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AskGlobalRecallRequest>,
) -> Result<Json<AskGlobalRecallResponse>, AppError> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("question is required".to_string()));
    }
    let _session = load_session(&state, &id).await?;
    let user_message = insert_recall_message(&state, &id, "user", question).await?;
    let answer = run_global_recall_agent(&state, &id, question).await?;
    let assistant_message = insert_recall_message(&state, &id, "assistant", &answer).await?;
    let now = Utc::now();
    sqlx::query("UPDATE global_recall_sessions SET updated_at = ?1 WHERE id = ?2")
        .bind(now.to_rfc3339())
        .bind(&id)
        .execute(state.db.pool())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(AskGlobalRecallResponse {
        user_message,
        assistant_message,
    }))
}

pub async fn resolve_reference(
    State(state): State<AppState>,
    Json(req): Json<ResolveGlobalReferenceRequest>,
) -> Result<Json<ResolveGlobalReferenceResponse>, AppError> {
    Ok(Json(resolve_reference_impl(&state, &req.reference).await?))
}

async fn build_open_work(state: &AppState) -> Result<GlobalOpenWorkResponse, AppError> {
    let conversations = state
        .db
        .list_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let projects = state
        .db
        .list_projects()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let project_by_id: HashMap<String, _> =
        projects.into_iter().map(|p| (p.id.clone(), p)).collect();
    let by_id: HashMap<String, Conversation> = conversations
        .iter()
        .cloned()
        .map(|c| (c.id.clone(), c))
        .collect();
    let active_ids: HashSet<String> = by_id.keys().cloned().collect();
    let mut predecessor_by_child: HashMap<String, String> = HashMap::new();
    for conv in &conversations {
        if let Some(child) = &conv.continued_in_conv_id {
            predecessor_by_child.insert(child.clone(), conv.id.clone());
        }
    }

    let mut visited = HashSet::new();
    let mut items = Vec::new();
    for conv in &conversations {
        if visited.contains(&conv.id) || predecessor_by_child.contains_key(&conv.id) {
            continue;
        }
        let mut member_ids = vec![conv.id.clone()];
        let mut cursor = conv;
        while let Some(next_id) = cursor.continued_in_conv_id.as_deref() {
            if !active_ids.contains(next_id) || member_ids.iter().any(|id| id == next_id) {
                break;
            }
            member_ids.push(next_id.to_string());
            cursor = &by_id[next_id];
        }
        for id in &member_ids {
            visited.insert(id.clone());
        }
        let members: Vec<Conversation> = member_ids
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
        if let Some(item) = project_item_from_members(&members) {
            items.push(item);
        }
    }

    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    items.truncate(OPEN_WORK_LIMIT);

    let mut grouped: HashMap<Option<String>, Vec<GlobalOpenWorkItem>> = HashMap::new();
    for item in items {
        grouped
            .entry(item.project_id.clone())
            .or_default()
            .push(item);
    }
    let mut groups: Vec<GlobalOpenWorkProject> = grouped
        .into_iter()
        .map(|(project_id, mut items)| {
            items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            let (project_name, canonical_path) = project_id
                .as_ref()
                .and_then(|id| project_by_id.get(id))
                .map_or_else(
                    || ("No project".to_string(), None),
                    |p| {
                        (
                            display_project_name(&p.canonical_path),
                            Some(p.canonical_path.clone()),
                        )
                    },
                );
            GlobalOpenWorkProject {
                project_id,
                project_name,
                canonical_path,
                items,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        let au = a.items.first().map(|i| i.updated_at);
        let bu = b.items.first().map(|i| i.updated_at);
        bu.cmp(&au)
            .then_with(|| a.project_name.cmp(&b.project_name))
    });

    Ok(GlobalOpenWorkResponse {
        generated_at: Utc::now(),
        groups,
    })
}

fn project_item_from_members(members: &[Conversation]) -> Option<GlobalOpenWorkItem> {
    if members.is_empty() {
        return None;
    }
    let root = &members[0];
    let current = members.last().expect("non-empty members");
    if should_suppress_item(current, members.len()) {
        return None;
    }
    let source = if members.len() >= 2 {
        GlobalOpenWorkSource::Chain
    } else {
        GlobalOpenWorkSource::Conversation
    };
    let mut signals = item_signals(current, members.len());
    let task_status = task_status_for(current);
    if let Some(status) = &task_status {
        if matches!(status.as_str(), "in-progress" | "ready" | "blocked") {
            signals.push(format!("task {status}"));
        }
    }
    let id = match source {
        GlobalOpenWorkSource::Chain => root.id.clone(),
        GlobalOpenWorkSource::Conversation => current.id.clone(),
    };
    let href = match source {
        GlobalOpenWorkSource::Chain => format!("/chains/{}", root.id),
        GlobalOpenWorkSource::Conversation => current
            .slug
            .as_ref()
            .map_or_else(|| format!("/c/{}", current.id), |s| format!("/c/{s}")),
    };
    let reference = match source {
        GlobalOpenWorkSource::Chain => format!("@chain:{}", root.id),
        GlobalOpenWorkSource::Conversation => format!("@conv:{}", current.id),
    };
    Some(GlobalOpenWorkItem {
        id,
        source,
        title: item_title(root, current, members.len()),
        project_id: current
            .project_id
            .clone()
            .or_else(|| root.project_id.clone()),
        current_conversation_id: current.id.clone(),
        current_conversation_slug: current.slug.clone(),
        root_conversation_id: root.id.clone(),
        root_conversation_slug: root.slug.clone(),
        updated_at: current.updated_at,
        mode: current.conv_mode.label().to_string(),
        state: current.state.variant_name().to_string(),
        task_id: current.conv_mode.task_id().map(str::to_string),
        task_title: current.conv_mode.task_title().map(str::to_string),
        task_status,
        branch_name: current.conv_mode.branch_name().map(str::to_string),
        base_branch: current.conv_mode.base_branch().map(str::to_string),
        worktree_path: current.conv_mode.worktree_path().map(str::to_string),
        member_count: members.len(),
        signals,
        href,
        reference,
    })
}

fn item_signals(conv: &Conversation, member_count: usize) -> Vec<String> {
    let mut signals = Vec::new();
    if Utc::now().signed_duration_since(conv.updated_at).num_days() <= RECENT_DAYS {
        signals.push("recent activity".to_string());
    }
    match conv.conv_mode {
        ConvMode::Work { .. } => signals.push("Work mode".to_string()),
        ConvMode::Branch { .. } => signals.push("Branch mode".to_string()),
        ConvMode::Explore { .. } | ConvMode::Direct => {}
    }
    match conv.state {
        ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. } => signals.push("active".to_string()),
        ConvState::AwaitingRecovery { .. } => signals.push("recovery needed".to_string()),
        ConvState::AwaitingTaskApproval { .. } => signals.push("task approval pending".to_string()),
        ConvState::AwaitingUserResponse { .. }
        | ConvState::AwaitingCommissionReviewApproval { .. }
        | ConvState::ContextExhausted { .. } => signals.push("needs action".to_string()),
        ConvState::Error { .. } => signals.push("error".to_string()),
        ConvState::Idle
        | ConvState::AwaitingContinuation { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::HandedOff { .. }
        | ConvState::Terminal => {}
    }
    if member_count >= 2 {
        signals.push(format!("{member_count}-conversation chain"));
    }
    if signals.is_empty() {
        signals.push("user-initiated open conversation".to_string());
    }
    signals
}

fn should_suppress_item(conv: &Conversation, member_count: usize) -> bool {
    if conv.archived || !conv.user_initiated {
        return true;
    }
    let old = Utc::now().signed_duration_since(conv.updated_at).num_days() > RECENT_DAYS;
    let low_value_mode = matches!(conv.conv_mode, ConvMode::Explore { .. } | ConvMode::Direct);
    let quiet_state = matches!(
        conv.state,
        ConvState::Terminal
            | ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::HandedOff { .. }
    );
    old && low_value_mode && quiet_state && member_count == 1
}

fn item_title(root: &Conversation, current: &Conversation, member_count: usize) -> String {
    if member_count >= 2 {
        root.chain_name
            .as_deref()
            .or(root.title.as_deref())
            .or(root.slug.as_deref())
            .unwrap_or(&root.id)
            .to_string()
    } else {
        current
            .title
            .as_deref()
            .or(current.slug.as_deref())
            .unwrap_or(&current.id)
            .to_string()
    }
}

fn display_project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn task_status_for(conv: &Conversation) -> Option<String> {
    let task_id = conv.conv_mode.task_id()?;
    let worktree = conv.conv_mode.worktree_path()?;
    let tasks_dir_name = taskmd_core::discover::discover_or_default(std::path::Path::new(worktree));
    let tasks_dir = std::path::Path::new(worktree).join(tasks_dir_name);
    let entries = std::fs::read_dir(tasks_dir).ok()?;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(parsed) = taskmd_core::filename::parse_filename(&name) else {
            continue;
        };
        if parsed.id == task_id {
            return Some(parsed.status.as_str().to_string());
        }
    }
    None
}

async fn load_session(state: &AppState, id: &str) -> Result<GlobalRecallSession, AppError> {
    sqlx::query(
        "SELECT id, title, created_at, updated_at FROM global_recall_sessions WHERE id = ?1",
    )
    .bind(id)
    .try_map(parse_session_row)
    .fetch_optional(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("global recall session not found".to_string()))
}

async fn load_session_messages(
    state: &AppState,
    id: &str,
) -> Result<Vec<GlobalRecallMessage>, AppError> {
    sqlx::query(
        "SELECT id, role, content, created_at FROM global_recall_messages WHERE session_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(id)
    .try_map(parse_message_row)
    .fetch_all(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))
}

async fn insert_recall_message(
    state: &AppState,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<GlobalRecallMessage, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let ordinal: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM global_recall_messages WHERE session_id = ?1",
    )
    .bind(session_id)
    .fetch_one(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query(
        "INSERT INTO global_recall_messages (id, session_id, ordinal, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(ordinal)
    .bind(role)
    .bind(content)
    .bind(now.to_rfc3339())
    .execute(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(GlobalRecallMessage {
        id,
        role: role.to_string(),
        content: content.to_string(),
        created_at: now,
    })
}

async fn run_global_recall_agent(
    state: &AppState,
    session_id: &str,
    question: &str,
) -> Result<String, AppError> {
    let (_model_id, service) = state
        .llm_registry
        .get_mid_tier_model()
        .or_else(|| {
            state
                .llm_registry
                .get_cheap_model()
                .map(|svc| ("cheap".to_string(), svc))
        })
        .ok_or_else(|| {
            AppError::Internal("no LLM model available for Global Recall".to_string())
        })?;
    let history = load_session_messages(state, session_id).await?;
    let mut messages = Vec::new();
    let mut orientation = String::from(
        "You are a read-only Phoenix Global Recall analyst. You may search and read Phoenix conversation history, inspect deterministic open-work projections, and resolve Phoenix references. Do not claim to have edited files or changed tasks. Cite sources using markdown links when tool results provide them; otherwise cite @conv:<id>, @chain:<id>, or message ids.\n\nRecent saved session context:\n",
    );
    for m in history.iter().rev().take(8).rev() {
        let _ = writeln!(orientation, "{}: {}", m.role, trim_chars(&m.content, 1200));
    }
    let _ = writeln!(orientation, "\nCurrent question: {question}");
    messages.push(LlmMessage {
        role: MessageRole::User,
        content: vec![ContentBlock::text(orientation)],
    });

    for turn in 0..MAX_RECALL_TURNS {
        let force_answer = turn + 1 == MAX_RECALL_TURNS;
        let request = LlmRequest {
            system: vec![SystemContent::new("Answer cross-conversation Phoenix strategy and handoff questions from read-only recalled evidence. Prefer concise synthesis, and include citations for factual claims.".to_string())],
            messages: messages.clone(),
            tools: if force_answer { vec![] } else { global_recall_tools() },
            max_tokens: Some(ANSWER_MAX_TOKENS),
            cache_key: PromptCacheKey::stable("global-recall-agent/v1"),
        };
        let resp = service
            .complete(&request)
            .await
            .map_err(|e| AppError::Internal(e.message))?;
        let tool_calls: Vec<_> = resp
            .tool_uses()
            .into_iter()
            .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
            .collect();
        if tool_calls.is_empty() || force_answer {
            return Ok(resp.text());
        }
        messages.push(LlmMessage {
            role: MessageRole::Assistant,
            content: resp.content.clone(),
        });
        let mut results = Vec::with_capacity(tool_calls.len());
        for (idx, (tool_use_id, name, input)) in tool_calls.iter().enumerate() {
            let (content, is_error) = if idx < 4 {
                execute_global_tool(state, name, input).await
            } else {
                (
                    "error: too many tool calls in one turn; issue fewer calls".to_string(),
                    true,
                )
            };
            results.push(ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content,
                images: vec![],
                is_error,
            });
        }
        messages.push(LlmMessage {
            role: MessageRole::User,
            content: results,
        });
    }
    Err(AppError::Internal(
        "global recall agent did not produce an answer".to_string(),
    ))
}

fn global_recall_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_conversations".to_string(),
            description: "Search all Phoenix conversation messages by relevance. Returns citable snippets with conversation/message references and app-local links.".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
            defer_loading: false,
        },
        ToolDefinition {
            name: "read_conversation".to_string(),
            description: "Read one source conversation transcript one bounded page at a time. Use conversation_id from search/open-work/reference results. If more content is available, call again with cursor.".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{"conversation_id":{"type":"string"},"cursor":{"type":"integer"}},"required":["conversation_id"]}),
            defer_loading: false,
        },
        ToolDefinition {
            name: "list_open_work".to_string(),
            description: "Read the deterministic Global Open Work projection grouped by project, including item references and explainable signals.".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            defer_loading: false,
        },
        ToolDefinition {
            name: "resolve_reference".to_string(),
            description: "Resolve @conv:<id>, @chain:<id>, @work:<id>, or app-local /c/... and /chains/... references.".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{"reference":{"type":"string"}},"required":["reference"]}),
            defer_loading: false,
        },
    ]
}

async fn execute_global_tool(
    state: &AppState,
    name: &str,
    input: &serde_json::Value,
) -> (String, bool) {
    match name {
        "search_conversations" => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return ("error: query is required".to_string(), true);
            }
            match state
                .message_retriever
                .retrieve(query, RetrievalScope::Global, SEARCH_TOP_K)
                .await
            {
                Ok(hits) if hits.is_empty() => ("No matching messages found.".to_string(), false),
                Ok(hits) => (format_global_search_hits(state, &hits).await, false),
                Err(e) => (format!("error: search failed: {e}"), true),
            }
        }
        "read_conversation" => {
            let conv_id = input
                .get("conversation_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .trim_start_matches('#');
            if conv_id.is_empty() {
                return ("error: conversation_id is required".to_string(), true);
            }
            let cursor = usize::try_from(
                input
                    .get("cursor")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(0);
            match state.db.get_conversation(conv_id).await {
                Ok(conv) => match state.db.get_messages(conv_id).await {
                    Ok(messages) => (read_conversation_page(&conv, &messages, cursor), false),
                    Err(e) => (format!("error: read failed: {e}"), true),
                },
                Err(e) => (format!("error: conversation not found: {e}"), true),
            }
        }
        "list_open_work" => match build_open_work(state).await {
            Ok(view) => (format_open_work_for_agent(&view), false),
            Err(e) => (format!("error: open work projection failed: {e:?}"), true),
        },
        "resolve_reference" => {
            let reference = input
                .get("reference")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            match resolve_reference_impl(state, reference).await {
                Ok(r) => (
                    serde_json::to_string_pretty(&r).unwrap_or_else(|_| "{}".to_string()),
                    false,
                ),
                Err(e) => (format!("error: {e:?}"), true),
            }
        }
        other => (format!("error: unknown tool {other}"), true),
    }
}

async fn format_global_search_hits(state: &AppState, hits: &[crate::db::RetrievedChunk]) -> String {
    let mut out = String::new();
    for hit in hits {
        let (title, href) = match state.db.get_conversation(&hit.conversation_id).await {
            Ok(conv) => {
                let title = conv
                    .title
                    .or(conv.slug.clone())
                    .unwrap_or_else(|| conv.id.clone());
                let href = conv
                    .slug
                    .map(|s| format!("/c/{s}#message-{}", hit.message_id));
                (title, href)
            }
            Err(_) => (hit.conversation_id.clone(), None),
        };
        let link =
            href.unwrap_or_else(|| format!("@conv:{} msg:{}", hit.conversation_id, hit.message_id));
        let _ = writeln!(
            out,
            "- [{} · {} · {}]({}) @conv:{} msg:{}: {}",
            title,
            hit.message_type,
            hit.created_at.format("%Y-%m-%d"),
            link,
            hit.conversation_id,
            hit.message_id,
            hit.snippet.trim()
        );
    }
    out
}

fn read_conversation_page(
    conv: &Conversation,
    messages: &[crate::db::Message],
    cursor: usize,
) -> String {
    let mut header = format!(
        "Conversation @conv:{} — {}\nlink: {}\nupdated: {}\n---\n",
        conv.id,
        conv.title
            .as_deref()
            .or(conv.slug.as_deref())
            .unwrap_or(&conv.id),
        conv.slug
            .as_ref()
            .map_or_else(|| format!("/c/{}", conv.id), |s| format!("/c/{s}")),
        conv.updated_at
    );
    let body = render_message_page(conv, messages, cursor);
    header.push_str(&body);
    header
}

fn render_message_page(
    conv: &Conversation,
    messages: &[crate::db::Message],
    cursor: usize,
) -> String {
    let end = cursor.saturating_add(READ_PAGE_CHARS);
    let mut out = String::new();
    let mut pos = 0usize;
    let mut has_more = false;
    'outer: for message in messages {
        let line = render_global_message_line(conv, message);
        for ch in line.chars() {
            if pos >= end {
                has_more = true;
                break 'outer;
            }
            if pos >= cursor {
                out.push(ch);
            }
            pos += 1;
        }
    }
    if out.is_empty() && !has_more {
        return "(end of conversation)".to_string();
    }
    if has_more {
        format!("{out}\n[… more content; call read_conversation again with cursor={end}]")
    } else {
        out
    }
}

fn render_global_message_line(conv: &Conversation, message: &crate::db::Message) -> String {
    let role = match message.message_type {
        MessageType::User => "User",
        MessageType::Agent => "Agent",
        MessageType::Tool => "Tool",
        MessageType::System => "System",
        MessageType::Error => "Error",
        MessageType::Continuation => "Continuation",
        MessageType::Skill => "Skill",
    };
    let href = conv.slug.as_ref().map_or_else(
        || format!("@conv:{} msg:{}", conv.id, message.message_id),
        |s| format!("/c/{s}#message-{}", message.message_id),
    );
    format!(
        "[{} · {} · {}]({}) @conv:{} msg:{}\n{}\n\n",
        role,
        message.created_at.format("%Y-%m-%d %H:%M"),
        message.message_id,
        href,
        conv.id,
        message.message_id,
        index_text(message).trim()
    )
}

fn format_open_work_for_agent(view: &GlobalOpenWorkResponse) -> String {
    let mut out = String::new();
    for group in &view.groups {
        let _ = writeln!(out, "Project: {}", group.project_name);
        for item in &group.items {
            let _ = writeln!(
                out,
                "- {} {} ({:?}) current @conv:{} link {} updated {} signals: {}",
                item.reference,
                item.title,
                item.source,
                item.current_conversation_id,
                item.href,
                item.updated_at,
                item.signals.join(", ")
            );
        }
    }
    out
}

async fn resolve_reference_impl(
    state: &AppState,
    raw: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let reference = raw.trim();
    let normalized = reference
        .strip_prefix("/c/")
        .map(|s| ("conv_slug", s.split('#').next().unwrap_or(s)))
        .or_else(|| {
            reference
                .strip_prefix("/chains/")
                .map(|s| ("chain", s.split('#').next().unwrap_or(s)))
        })
        .or_else(|| reference.strip_prefix("@conv:").map(|s| ("conv", s)))
        .or_else(|| reference.strip_prefix("@chain:").map(|s| ("chain", s)))
        .or_else(|| reference.strip_prefix("@work:").map(|s| ("work", s)))
        .ok_or_else(|| AppError::BadRequest("unsupported reference syntax".to_string()))?;
    match normalized {
        ("conv", id) => resolve_conversation_id(state, id).await,
        ("conv_slug", slug) => {
            let conv = state
                .db
                .get_conversation_by_slug(slug)
                .await
                .map_err(map_db_not_found)?;
            Ok(resolve_conversation(conv))
        }
        ("chain", id) => resolve_chain(state, id).await,
        ("work", id) => resolve_work(state, id).await,
        _ => Err(AppError::BadRequest(
            "unsupported reference syntax".to_string(),
        )),
    }
}

async fn resolve_conversation_id(
    state: &AppState,
    id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let conv = state
        .db
        .get_conversation(id)
        .await
        .map_err(map_db_not_found)?;
    Ok(resolve_conversation(conv))
}

fn resolve_conversation(conv: Conversation) -> ResolveGlobalReferenceResponse {
    let href = conv.slug.as_ref().map(|s| format!("/c/{s}"));
    let title = conv.title.clone().or(conv.slug.clone());
    ResolveGlobalReferenceResponse {
        kind: "conversation".to_string(),
        id: conv.id.clone(),
        href,
        title: title.clone(),
        summary: format!(
            "conversation {} updated {} state {}",
            title.unwrap_or(conv.id),
            conv.updated_at,
            conv.state.variant_name()
        ),
    }
}

async fn resolve_chain(
    state: &AppState,
    root_id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let root = state
        .db
        .get_conversation(root_id)
        .await
        .map_err(map_db_not_found)?;
    let members = state
        .db
        .chain_members_forward(root_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(ResolveGlobalReferenceResponse {
        kind: "chain".to_string(),
        id: root_id.to_string(),
        href: Some(format!("/chains/{root_id}")),
        title: root
            .chain_name
            .clone()
            .or(root.title.clone())
            .or(root.slug.clone()),
        summary: format!(
            "chain rooted at @conv:{root_id} with {} member(s)",
            members.len()
        ),
    })
}

async fn resolve_work(
    state: &AppState,
    id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let view = build_open_work(state).await?;
    for group in view.groups {
        for item in group.items {
            if item.id == id || item.reference.ends_with(id) {
                return Ok(ResolveGlobalReferenceResponse {
                    kind: "work".to_string(),
                    id: item.id,
                    href: Some(item.href),
                    title: Some(item.title),
                    summary: format!(
                        "open work item in {}: {}",
                        group.project_name,
                        item.signals.join(", ")
                    ),
                });
            }
        }
    }
    Err(AppError::NotFound("open work item not found".to_string()))
}

fn map_db_not_found(e: DbError) -> AppError {
    match e {
        DbError::ConversationNotFound(_) => {
            AppError::NotFound("reference target not found".to_string())
        }
        other @ (DbError::Sqlx(_)
        | DbError::MessageNotFound(_)
        | DbError::SlugExists(_)
        | DbError::Serialization(_)
        | DbError::ForkProposalConflict(_)) => AppError::Internal(other.to_string()),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn parse_session_row(row: sqlx::sqlite::SqliteRow) -> Result<GlobalRecallSession, sqlx::Error> {
    Ok(GlobalRecallSession {
        id: row.try_get("id")?,
        title: row.try_get("title")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        updated_at: parse_datetime(&row.try_get::<String, _>("updated_at")?),
    })
}

#[allow(clippy::needless_pass_by_value)]
fn parse_message_row(row: sqlx::sqlite::SqliteRow) -> Result<GlobalRecallMessage, sqlx::Error> {
    Ok(GlobalRecallMessage {
        id: row.try_get("id")?,
        role: row.try_get("role")?,
        content: row.try_get("content")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
    })
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

fn trim_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}
