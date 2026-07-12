use super::AppState;
use crate::db::MessageContent;
use crate::db::{ConvMode, Conversation, DbError, MessageType, RetrievalScope};
use crate::state_machine::ConvState;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, PromptCacheKey, SystemContent,
    ToolDefinition,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock, Mutex};

use super::handlers::AppError;

const RECENT_DAYS: i64 = 14;
const SEARCH_TOP_K: usize = 10;
const READ_PAGE_CHARS: usize = 7000;
const MAX_RECALL_TURNS: usize = 6;
const ANSWER_MAX_TOKENS: u32 = 3072;
const READ_MESSAGE_BATCH: i64 = 64;
const READ_TARGET_SIDE_MESSAGES: i64 = 32;
const RECALL_SESSION_PAGE: i64 = 50;
const RECALL_MESSAGE_PAGE: i64 = 100;
const OPEN_WORK_PAGE: usize = 100;

static RECALL_SESSION_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Serialize)]
pub struct GlobalOpenWorkResponse {
    pub generated_at: DateTime<Utc>,
    pub groups: Vec<GlobalOpenWorkProject>,
    pub has_more: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct ConversationReadTarget {
    conversation_id: String,
    message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
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
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct GlobalRecallSessionResponse {
    pub session: GlobalRecallSession,
    pub messages: Vec<GlobalRecallMessage>,
    pub older_cursor: Option<i64>,
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

#[derive(Debug, Deserialize)]
pub struct OpenWorkPageQuery {
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Deserialize)]
pub struct SessionPageQuery {
    #[serde(default)]
    pub offset: i64,
}

#[derive(Debug, Deserialize)]
pub struct MessagePageQuery {
    pub before: Option<i64>,
}

pub async fn open_work(
    State(state): State<AppState>,
    Query(page): Query<OpenWorkPageQuery>,
) -> Result<Json<GlobalOpenWorkResponse>, AppError> {
    let view = build_open_work(&state).await?;
    Ok(Json(paginate_open_work(view, page.offset, OPEN_WORK_PAGE)))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Query(page): Query<SessionPageQuery>,
) -> Result<Json<GlobalRecallSessionsResponse>, AppError> {
    let mut sessions = sqlx::query(
        "SELECT id, title, created_at, updated_at FROM global_recall_sessions
         ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
    )
    .bind(RECALL_SESSION_PAGE + 1)
    .bind(page.offset.max(0))
    .try_map(parse_session_row)
    .fetch_all(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let has_more = sessions.len() > usize::try_from(RECALL_SESSION_PAGE).unwrap_or(50);
    sessions.truncate(usize::try_from(RECALL_SESSION_PAGE).unwrap_or(50));
    Ok(Json(GlobalRecallSessionsResponse { sessions, has_more }))
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
    Query(page): Query<MessagePageQuery>,
) -> Result<Json<GlobalRecallSessionResponse>, AppError> {
    let session = load_session(&state, &id).await?;
    let (messages, older_cursor) = load_session_message_page(&state, &id, page.before).await?;
    Ok(Json(GlobalRecallSessionResponse {
        session,
        messages,
        older_cursor,
    }))
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
    let session_lock = recall_session_lock(&id)?;
    let _guard = session_lock.lock().await;
    let answer = run_global_recall_agent(&state, &id, question).await?;
    let (user_message, assistant_message) =
        insert_recall_turn(&state.db, &id, question, &answer).await?;
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

fn recall_session_lock(session_id: &str) -> Result<Arc<tokio::sync::Mutex<()>>, AppError> {
    let mut locks = RECALL_SESSION_LOCKS.lock().map_err(|_| {
        AppError::Internal("Global Recall session lock registry is poisoned".to_string())
    })?;
    Ok(locks
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}

async fn build_open_work(state: &AppState) -> Result<GlobalOpenWorkResponse, AppError> {
    let now = Utc::now();
    let conversations = state
        .db
        .list_all_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let conversations: Vec<_> = conversations
        .into_iter()
        .filter(|conversation| conversation.kind == crate::db::ConversationKind::Standard)
        .collect();
    let projects = state
        .db
        .list_projects()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let project_by_id: HashMap<String, _> =
        projects.into_iter().map(|p| (p.id.clone(), p)).collect();
    let chains = group_conversation_chains(&conversations);
    let mut items = Vec::new();
    for members in chains {
        if let Some(item) = project_item_from_members(&members, now).await? {
            items.push(item);
        }
    }

    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

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
        generated_at: now,
        groups,
        has_more: false,
    })
}

fn paginate_open_work(
    view: GlobalOpenWorkResponse,
    offset: usize,
    limit: usize,
) -> GlobalOpenWorkResponse {
    let mut flattened = Vec::new();
    for group in view.groups {
        for item in group.items {
            flattened.push((
                group.project_id.clone(),
                group.project_name.clone(),
                group.canonical_path.clone(),
                item,
            ));
        }
    }
    let has_more = offset.saturating_add(limit) < flattened.len();
    let mut groups: Vec<GlobalOpenWorkProject> = Vec::new();
    for (project_id, project_name, canonical_path, item) in
        flattened.into_iter().skip(offset).take(limit)
    {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.project_id == project_id)
        {
            group.items.push(item);
        } else {
            groups.push(GlobalOpenWorkProject {
                project_id,
                project_name,
                canonical_path,
                items: vec![item],
            });
        }
    }
    GlobalOpenWorkResponse {
        generated_at: view.generated_at,
        groups,
        has_more,
    }
}

fn group_conversation_chains(conversations: &[Conversation]) -> Vec<Vec<Conversation>> {
    let by_id: HashMap<String, Conversation> = conversations
        .iter()
        .cloned()
        .map(|c| (c.id.clone(), c))
        .collect();
    let all_ids: HashSet<&str> = by_id.keys().map(String::as_str).collect();
    let predecessor_ids: HashSet<&str> = conversations
        .iter()
        .filter_map(|c| c.continued_in_conv_id.as_deref())
        .filter(|id| all_ids.contains(id))
        .collect();
    let mut visited = HashSet::new();
    let mut chains = Vec::new();

    for conversation in conversations {
        if visited.contains(&conversation.id) || predecessor_ids.contains(conversation.id.as_str())
        {
            continue;
        }
        let mut members = Vec::new();
        let mut cursor = conversation;
        loop {
            if !visited.insert(cursor.id.clone()) {
                break;
            }
            members.push(cursor.clone());
            let Some(next_id) = cursor.continued_in_conv_id.as_deref() else {
                break;
            };
            let Some(next) = by_id.get(next_id) else {
                break;
            };
            cursor = next;
        }
        chains.push(members);
    }

    for conversation in conversations {
        if visited.insert(conversation.id.clone()) {
            chains.push(vec![conversation.clone()]);
        }
    }
    chains
}

async fn project_item_from_members(
    members: &[Conversation],
    now: DateTime<Utc>,
) -> Result<Option<GlobalOpenWorkItem>, AppError> {
    if members.is_empty() {
        return Ok(None);
    }
    let root = &members[0];
    let current = members.last().expect("non-empty members");
    if is_intrinsically_closed(current) {
        return Ok(None);
    }
    let task_status = task_status_for(current).await?;
    if !is_open_work_candidate(current, task_status.as_deref(), now) {
        return Ok(None);
    }
    let source = if members.len() >= 2 {
        GlobalOpenWorkSource::Chain
    } else {
        GlobalOpenWorkSource::Conversation
    };
    let mut signals = item_signals(current, members.len(), now);
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
        GlobalOpenWorkSource::Conversation => conversation_href(current),
    };
    let reference = format!("@work:{id}");
    Ok(Some(GlobalOpenWorkItem {
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
    }))
}

fn item_signals(conv: &Conversation, member_count: usize, now: DateTime<Utc>) -> Vec<String> {
    let mut signals = Vec::new();
    if now.signed_duration_since(conv.updated_at).num_days() <= RECENT_DAYS {
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
        | ConvState::AwaitingContinuation { .. }
        | ConvState::ContextExhausted { .. } => signals.push("needs action".to_string()),
        ConvState::Error { .. } => signals.push("error".to_string()),
        ConvState::Provisioning { .. } => signals.push("provisioning".to_string()),
        ConvState::CreationFailed { .. } => signals.push("creation failed".to_string()),
        ConvState::CreationCancelled { .. } => signals.push("creation cancelled".to_string()),
        ConvState::Idle
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::HandedOff { .. }
        | ConvState::Terminal => {}
    }
    if member_count >= 2 {
        signals.push(format!("{member_count}-conversation chain"));
    }
    signals
}

fn is_open_task_status(status: &str) -> bool {
    matches!(status, "in-progress" | "ready" | "blocked")
}

fn is_closed_task_status(status: &str) -> bool {
    matches!(status, "done" | "wont-do")
}

fn has_runtime_open_evidence(state: &ConvState) -> bool {
    matches!(
        state,
        ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::ContextExhausted { .. }
            | ConvState::Error { .. }
    )
}

fn is_closed_runtime_state(state: &ConvState) -> bool {
    matches!(
        state,
        ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::CreationFailed { .. }
            | ConvState::CreationCancelled { .. }
            | ConvState::HandedOff { .. }
            | ConvState::Terminal
    )
}

fn is_intrinsically_closed(conversation: &Conversation) -> bool {
    conversation.archived
        || !conversation.user_initiated
        || is_closed_runtime_state(&conversation.state)
}

fn is_open_work_candidate(
    conversation: &Conversation,
    task_status: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    if is_intrinsically_closed(conversation) || task_status.is_some_and(is_closed_task_status) {
        return false;
    }
    if has_runtime_open_evidence(&conversation.state) {
        return true;
    }
    match conversation.conv_mode {
        ConvMode::Work { .. } => task_status.is_some_and(is_open_task_status),
        ConvMode::Branch { .. } | ConvMode::Explore { .. } | ConvMode::Direct => {
            now.signed_duration_since(conversation.updated_at)
                .num_days()
                <= RECENT_DAYS
        }
    }
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

async fn task_status_for(conv: &Conversation) -> Result<Option<String>, AppError> {
    let Some(task_id) = conv.conv_mode.task_id().map(str::to_string) else {
        return Ok(None);
    };
    let Some(worktree) = conv.conv_mode.worktree_path().map(str::to_string) else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || task_status_from_disk(&worktree, &task_id))
        .await
        .map_err(|e| AppError::Internal(format!("task status worker failed: {e}")))
}

fn task_status_from_disk(worktree: &str, task_id: &str) -> Option<String> {
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
    let mut messages = sqlx::query(
        "SELECT id, role, content, created_at FROM global_recall_messages
         WHERE session_id = ?1 ORDER BY ordinal DESC LIMIT 8",
    )
    .bind(id)
    .try_map(parse_message_row)
    .fetch_all(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    messages.reverse();
    Ok(messages)
}

async fn load_session_message_page(
    state: &AppState,
    id: &str,
    before: Option<i64>,
) -> Result<(Vec<GlobalRecallMessage>, Option<i64>), AppError> {
    let mut rows = sqlx::query(
        "SELECT ordinal, id, role, content, created_at FROM global_recall_messages
         WHERE session_id = ?1 AND (?2 IS NULL OR ordinal < ?2)
         ORDER BY ordinal DESC LIMIT ?3",
    )
    .bind(id)
    .bind(before)
    .bind(RECALL_MESSAGE_PAGE + 1)
    .try_map(parse_message_with_ordinal_row)
    .fetch_all(state.db.pool())
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let page_size = usize::try_from(RECALL_MESSAGE_PAGE).unwrap_or(100);
    let has_more = rows.len() > page_size;
    rows.truncate(page_size);
    rows.reverse();
    let older_cursor = has_more
        .then(|| rows.first().map(|(ordinal, _)| *ordinal))
        .flatten();
    Ok((
        rows.into_iter().map(|(_, message)| message).collect(),
        older_cursor,
    ))
}

async fn insert_recall_turn(
    db: &crate::db::Database,
    session_id: &str,
    question: &str,
    answer: &str,
) -> Result<(GlobalRecallMessage, GlobalRecallMessage), AppError> {
    let user = GlobalRecallMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: "user".to_string(),
        content: question.to_string(),
        created_at: Utc::now(),
    };
    let assistant = GlobalRecallMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".to_string(),
        content: answer.to_string(),
        created_at: Utc::now(),
    };
    for attempt in 0..3 {
        match insert_recall_turn_once(db, session_id, &user, &assistant).await {
            Ok(()) => return Ok((user, assistant)),
            Err(e) if attempt < 2 && is_recall_ordinal_conflict(&e) => (),
            Err(e) => return Err(e),
        }
    }
    Err(AppError::Internal(
        "failed to insert Global Recall turn".to_string(),
    ))
}

async fn insert_recall_turn_once(
    db: &crate::db::Database,
    session_id: &str,
    user: &GlobalRecallMessage,
    assistant: &GlobalRecallMessage,
) -> Result<(), AppError> {
    let mut tx = db
        .pool()
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    insert_recall_message_in_tx(&mut tx, session_id, user).await?;
    insert_recall_message_in_tx(&mut tx, session_id, assistant).await?;
    sqlx::query("UPDATE global_recall_sessions SET updated_at = ?1 WHERE id = ?2")
        .bind(assistant.created_at.to_rfc3339())
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

fn is_recall_ordinal_conflict(error: &AppError) -> bool {
    matches!(error, AppError::Internal(message) if message.contains("global_recall_messages.session_id") || message.contains("UNIQUE constraint failed"))
}

async fn insert_recall_message_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    message: &GlobalRecallMessage,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO global_recall_messages (id, session_id, ordinal, role, content, created_at)
         SELECT ?1, ?2, COALESCE(MAX(ordinal) + 1, 0), ?3, ?4, ?5
         FROM global_recall_messages WHERE session_id = ?2",
    )
    .bind(&message.id)
    .bind(session_id)
    .bind(&message.role)
    .bind(&message.content)
    .bind(message.created_at.to_rfc3339())
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(())
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
    for m in &history {
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
            input_schema: serde_json::json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0}}}),
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
            if !global_index_is_fresh(state) {
                return (
                    "search unavailable: the global message index is still warming; try again after startup reconciliation completes".to_string(),
                    true,
                );
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
            let conv_ref = input
                .get("conversation_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            if conv_ref.is_empty() {
                return ("error: conversation_id is required".to_string(), true);
            }
            let target = match resolve_conversation_read_target(state, conv_ref).await {
                Ok(target) => target,
                Err(e) => return (format!("error: {e}"), true),
            };
            let cursor = usize::try_from(
                input
                    .get("cursor")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(0);
            match state.db.get_conversation(&target.conversation_id).await {
                Ok(conv) => {
                    let result = if let Some(message_id) = target.message_id.as_deref() {
                        read_conversation_around_message(&state.db, &conv, message_id).await
                    } else {
                        read_conversation_page(&state.db, &conv, cursor).await
                    };
                    match result {
                        Ok(page) => (page, false),
                        Err(e) => (format!("error: read failed: {e}"), true),
                    }
                }
                Err(e) => (format!("error: conversation not found: {e}"), true),
            }
        }
        "list_open_work" => {
            let offset = usize::try_from(
                input
                    .get("offset")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(usize::MAX);
            match build_open_work(state).await {
                Ok(view) => {
                    let page = paginate_open_work(view, offset, OPEN_WORK_PAGE);
                    (format_open_work_for_agent(&page, offset), false)
                }
                Err(e) => (format!("error: open work projection failed: {e:?}"), true),
            }
        }
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

async fn resolve_conversation_read_target(
    state: &AppState,
    raw: &str,
) -> Result<ConversationReadTarget, String> {
    let reference = raw.trim().trim_start_matches('#');
    if let Some(rest) = reference.strip_prefix("@conv:") {
        let (id, message_id) = parse_conv_handle(rest);
        if id.is_empty() {
            return Err("conversation reference is missing an id".to_string());
        }
        return Ok(ConversationReadTarget {
            conversation_id: id.to_string(),
            message_id: message_id.map(str::to_string),
        });
    }
    if let Some(rest) = reference.strip_prefix("/c/") {
        let (slug, fragment) = split_fragment(rest);
        let conv = load_conversation_by_slug_or_id(state, slug)
            .await
            .map_err(|e| format!("conversation reference not found: {e:?}"))?;
        return Ok(ConversationReadTarget {
            conversation_id: conv.id,
            message_id: fragment.and_then(message_id_fragment).map(str::to_string),
        });
    }
    let (id, message_id) = parse_conv_handle(reference);
    if id.is_empty() {
        Err("conversation reference is missing an id".to_string())
    } else {
        Ok(ConversationReadTarget {
            conversation_id: id.to_string(),
            message_id: message_id.map(str::to_string),
        })
    }
}

fn global_index_is_fresh(state: &AppState) -> bool {
    state.message_retriever.index_reconciled()
}

async fn format_global_search_hits(state: &AppState, hits: &[crate::db::RetrievedChunk]) -> String {
    let mut out = String::new();
    for hit in hits {
        let (title, href) = match state.db.get_conversation(&hit.conversation_id).await {
            Ok(conv) => {
                let title = conv
                    .title
                    .clone()
                    .or(conv.slug.clone())
                    .unwrap_or_else(|| conv.id.clone());
                let href = Some(conversation_message_href(
                    &conv,
                    Some((&hit.message_id, hit.message_type)),
                ));
                (title, href)
            }
            Err(_) => (hit.conversation_id.clone(), None),
        };
        let link =
            href.unwrap_or_else(|| format!("@conv:{} msg:{}", hit.conversation_id, hit.message_id));
        let _ = writeln!(
            out,
            "- [{} · {} · {}]({}) @conv:{} msg:{} — {}",
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

async fn read_conversation_page(
    db: &crate::db::Database,
    conv: &Conversation,
    cursor: usize,
) -> Result<String, DbError> {
    let mut header = format!(
        "Conversation @conv:{} — {}\nlink: {}\nupdated: {}\n---\n",
        conv.id,
        conv.title
            .as_deref()
            .or(conv.slug.as_deref())
            .unwrap_or(&conv.id),
        conversation_href(conv),
        conv.updated_at
    );
    let body = render_message_page(db, conv, cursor).await?;
    header.push_str(&body);
    Ok(header)
}

async fn read_conversation_around_message(
    db: &crate::db::Database,
    conv: &Conversation,
    message_id: &str,
) -> Result<String, DbError> {
    let target = db.get_message_by_id(message_id).await?;
    if target.conversation_id != conv.id {
        return Err(DbError::MessageNotFound(message_id.to_string()));
    }
    let probe_limit = READ_TARGET_SIDE_MESSAGES.saturating_add(1);
    let (mut before, mut after) = db
        .get_messages_around(&conv.id, target.sequence_id, probe_limit, probe_limit)
        .await?;
    let side_limit = usize::try_from(READ_TARGET_SIDE_MESSAGES).unwrap_or(0);
    let has_more_before = before.len() > side_limit;
    let has_more_after = after.len() > side_limit;
    if has_more_before {
        before.remove(0);
    }
    after.truncate(side_limit);
    let mut messages = before;
    messages.push(target);
    messages.extend(after);

    let mut out = format!(
        "Conversation @conv:{} — {}\nlink: {}\nupdated: {}\ntarget_message: {}\nhas_more_before: {}\nhas_more_after: {}\n---\n",
        conv.id,
        conv.title
            .as_deref()
            .or(conv.slug.as_deref())
            .unwrap_or(&conv.id),
        conversation_href(conv),
        conv.updated_at,
        message_id,
        has_more_before,
        has_more_after,
    );
    for message in messages {
        if !message_is_hidden(&message) {
            out.push_str(&render_global_message_line(conv, &message));
        }
    }
    Ok(out)
}

async fn render_message_page(
    db: &crate::db::Database,
    conv: &Conversation,
    cursor: usize,
) -> Result<String, DbError> {
    let end = cursor.saturating_add(READ_PAGE_CHARS);
    let mut out = String::new();
    let mut pos = 0usize;
    let mut has_more = false;
    let mut after_sequence = 0;
    loop {
        let messages = db
            .get_messages_after_limited(&conv.id, after_sequence, READ_MESSAGE_BATCH)
            .await?;
        if messages.is_empty() {
            break;
        }
        for message in messages {
            after_sequence = message.sequence_id;
            if message_is_hidden(&message) {
                continue;
            }
            let line = render_global_message_line(conv, &message);
            for ch in line.chars() {
                if pos >= end {
                    has_more = true;
                    break;
                }
                if pos >= cursor {
                    out.push(ch);
                }
                pos += 1;
            }
            if has_more {
                break;
            }
        }
        if has_more {
            break;
        }
    }
    if out.is_empty() && !has_more {
        return Ok("(end of conversation)".to_string());
    }
    if has_more {
        Ok(format!(
            "{out}\n[… more content; call read_conversation again with cursor={end}]"
        ))
    } else {
        Ok(out)
    }
}

fn message_is_hidden(message: &crate::db::Message) -> bool {
    message
        .display_data
        .as_ref()
        .and_then(|d| d.get("hidden"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn conversation_href(conv: &Conversation) -> String {
    format!("/c/{}", conv.slug.as_deref().unwrap_or(&conv.id))
}

fn conversation_message_href(conv: &Conversation, message: Option<(&str, MessageType)>) -> String {
    let base = conversation_href(conv);
    match message {
        Some((message_id, message_type)) if message_type_has_rendered_anchor(message_type) => {
            format!("{base}#message-{message_id}")
        }
        _ => base,
    }
}

fn message_type_has_rendered_anchor(message_type: MessageType) -> bool {
    matches!(
        message_type,
        MessageType::User | MessageType::Agent | MessageType::Skill
    )
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
    let href = conversation_message_href(conv, Some((&message.message_id, message.message_type)));
    format!(
        "[{} · {} · {}]({}) @conv:{} msg:{}\n{}\n\n",
        role,
        message.created_at.format("%Y-%m-%d %H:%M"),
        message.message_id,
        href,
        conv.id,
        message.message_id,
        render_full_message_text(message).trim()
    )
}

fn render_full_message_text(message: &crate::db::Message) -> String {
    match &message.content {
        MessageContent::User(c) => {
            let mut text = c.llm_text().to_string();
            for f in &c.files {
                text.push('\n');
                text.push_str(&f.llm_context_tag());
            }
            if !c.images.is_empty() {
                tracing::debug!(
                    n = c.images.len(),
                    "global recall read_conversation: dropping user-message images — image recall is unsupported",
                );
                let _ = write!(
                    text,
                    "\n[{} image(s) attached to this message are not shown — Global Recall reads text only]",
                    c.images.len()
                );
            }
            text
        }
        MessageContent::Agent(blocks) => blocks
            .iter()
            .map(ContentBlock::render_text)
            .collect::<Vec<_>>()
            .join("\n"),
        MessageContent::Tool(c) => {
            if c.images.is_empty() {
                c.content.clone()
            } else {
                tracing::debug!(
                    tool_use_id = %c.tool_use_id,
                    n = c.images.len(),
                    "global recall read_conversation: dropping tool-result images — image recall is unsupported",
                );
                format!(
                    "{}\n[{} image(s) in this tool result are not shown — Global Recall reads text only]",
                    c.content,
                    c.images.len()
                )
            }
        }
        MessageContent::System(c) => c.text.clone(),
        MessageContent::Error(c) => c.message.clone(),
        MessageContent::Continuation(c) => c.summary.clone(),
        MessageContent::Skill(c) => {
            let mut body = format!("/{} {}\n{}", c.name, c.trigger, c.body);
            for f in &c.files {
                body.push('\n');
                body.push_str(&f.llm_context_tag());
            }
            body
        }
    }
}

fn format_open_work_for_agent(view: &GlobalOpenWorkResponse, offset: usize) -> String {
    let item_count = view
        .groups
        .iter()
        .map(|group| group.items.len())
        .sum::<usize>();
    let next_offset = view.has_more.then(|| offset.saturating_add(item_count));
    let mut out = format!(
        "Open work page offset {offset}\nhas_more: {}\nnext_offset: {}\n",
        view.has_more,
        next_offset.map_or_else(|| "none".to_string(), |value| value.to_string()),
    );
    if item_count == 0 {
        out.push_str("No active work found.\n");
        return out;
    }
    for group in &view.groups {
        let _ = writeln!(
            out,
            "Project: {} id {} path {}",
            group.project_name,
            group.project_id.as_deref().unwrap_or("none"),
            group.canonical_path.as_deref().unwrap_or("none")
        );
        for item in &group.items {
            let _ = writeln!(
                out,
                "- {} {} ({:?}) current @conv:{} root @conv:{} link {} updated {} mode {} state {} task {} {} {} branch {} base {} worktree {} signals: {}",
                item.reference,
                item.title,
                item.source,
                item.current_conversation_id,
                item.root_conversation_id,
                item.href,
                item.updated_at,
                item.mode,
                item.state,
                item.task_id.as_deref().unwrap_or("none"),
                item.task_status.as_deref().unwrap_or(""),
                item.task_title.as_deref().unwrap_or(""),
                item.branch_name.as_deref().unwrap_or("none"),
                item.base_branch.as_deref().unwrap_or("none"),
                item.worktree_path.as_deref().unwrap_or("none"),
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
    if let Some(rest) = reference.strip_prefix("/c/") {
        let (slug, fragment) = split_fragment(rest);
        let conv = load_conversation_by_slug_or_id(state, slug).await?;
        if let Some(message_id) = fragment.and_then(message_id_fragment) {
            return resolve_message(state, conv, message_id).await;
        }
        return Ok(resolve_conversation(conv));
    }
    if let Some(rest) = reference.strip_prefix("/chains/") {
        let (id, _) = split_fragment(rest);
        return resolve_chain(state, id).await;
    }
    if let Some(rest) = reference.strip_prefix("@conv:") {
        let (id, message_id) = parse_conv_handle(rest);
        let conv = state
            .db
            .get_conversation(id)
            .await
            .map_err(map_db_not_found)?;
        if let Some(message_id) = message_id {
            return resolve_message(state, conv, message_id).await;
        }
        return Ok(resolve_conversation(conv));
    }
    if let Some(rest) = reference.strip_prefix("@chain:") {
        let (id, _) = split_fragment(rest);
        return resolve_chain(state, first_token(id)).await;
    }
    if let Some(rest) = reference.strip_prefix("@work:") {
        let (id, _) = split_fragment(rest);
        return resolve_work(state, first_token(id)).await;
    }
    Err(AppError::BadRequest(
        "unsupported reference syntax".to_string(),
    ))
}

fn split_fragment(s: &str) -> (&str, Option<&str>) {
    s.split_once('#')
        .map_or((s, None), |(base, fragment)| (base, Some(fragment)))
}

fn first_token(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or(s)
}

fn message_id_fragment(fragment: &str) -> Option<&str> {
    fragment
        .strip_prefix("message-")
        .filter(|id| !id.is_empty())
}

fn handle_token(s: &str) -> &str {
    s.trim_end_matches([':', ',', ';', '.', ')', ']', '}'])
}

fn parse_conv_handle(rest: &str) -> (&str, Option<&str>) {
    let (id_part, fragment) = split_fragment(rest);
    let mut parts = id_part.split_whitespace();
    let id = parts.next().unwrap_or(id_part);
    if id.is_empty() {
        return (id, None);
    }
    let message_id = parts
        .next()
        .and_then(|part| {
            part.strip_prefix("msg:")
                .map(handle_token)
                .filter(|id| !id.is_empty())
                .or_else(|| {
                    (part == "msg:")
                        .then(|| parts.next().map(handle_token))
                        .flatten()
                })
        })
        .or_else(|| fragment.and_then(message_id_fragment));
    (id, message_id)
}

async fn load_conversation_by_slug_or_id(
    state: &AppState,
    slug_or_id: &str,
) -> Result<Conversation, AppError> {
    match state.db.get_conversation_by_slug(slug_or_id).await {
        Ok(conv) => Ok(conv),
        Err(DbError::ConversationNotFound(_)) => state
            .db
            .get_conversation(slug_or_id)
            .await
            .map_err(map_db_not_found),
        Err(e) => Err(map_db_not_found(e)),
    }
}

fn resolve_conversation(conv: Conversation) -> ResolveGlobalReferenceResponse {
    let href = Some(conversation_href(&conv));
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

async fn resolve_message(
    state: &AppState,
    conv: Conversation,
    message_id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let message = state
        .db
        .get_message_by_id(message_id)
        .await
        .map_err(|_| AppError::NotFound("message reference target not found".to_string()))?;
    if message.conversation_id != conv.id {
        return Err(AppError::NotFound(
            "message reference target not found".to_string(),
        ));
    }
    if message_is_hidden(&message) {
        return Err(AppError::NotFound(
            "message reference target not found".to_string(),
        ));
    }
    let href = Some(conversation_message_href(
        &conv,
        Some((&message.message_id, message.message_type)),
    ));
    let title = conv.title.clone().or(conv.slug.clone());
    Ok(ResolveGlobalReferenceResponse {
        kind: "message".to_string(),
        id: message.message_id.clone(),
        href,
        title,
        summary: format!(
            "{} message {} in @conv:{} at {}: {}",
            message.message_type,
            message.message_id,
            conv.id,
            message.created_at,
            trim_chars(render_full_message_text(&message).trim(), 240)
        ),
    })
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
    let resolved_root = state
        .db
        .chain_root_of(root_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if resolved_root.as_deref() != Some(root_id) || members.len() < 2 {
        return Err(AppError::NotFound(
            "chain reference target not found".to_string(),
        ));
    }
    let member_refs = members
        .iter()
        .map(|member| format!("@conv:{member}"))
        .collect::<Vec<_>>()
        .join(", ");
    let current = members.last().map_or_else(
        || format!("@conv:{root_id}"),
        |member| format!("@conv:{member}"),
    );
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
            "chain rooted at @conv:{root_id} with {} member(s); current/latest {}; ordered members: {}",
            members.len(),
            current,
            member_refs
        ),
    })
}

async fn resolve_work(
    state: &AppState,
    id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let root = state
        .db
        .get_conversation(id)
        .await
        .map_err(map_db_not_found)?;
    let chain_root = state
        .db
        .chain_root_of(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if chain_root.as_deref().is_some_and(|root_id| root_id != id) {
        return Err(AppError::NotFound(
            "work reference target not found".to_string(),
        ));
    }
    let member_ids = state
        .db
        .chain_members_forward(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut members = Vec::with_capacity(member_ids.len().max(1));
    if member_ids.len() >= 2 {
        for member_id in member_ids {
            members.push(
                state
                    .db
                    .get_conversation(&member_id)
                    .await
                    .map_err(map_db_not_found)?,
            );
        }
    } else {
        members.push(root.clone());
    }
    let current = members.last().unwrap_or(&root);
    let task_status = task_status_for(current).await?;
    let status = if current.archived {
        "archived"
    } else if is_open_work_candidate(current, task_status.as_deref(), Utc::now()) {
        "open"
    } else {
        "closed"
    };
    let projects = state
        .db
        .list_projects()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let project = current
        .project_id
        .as_ref()
        .or(root.project_id.as_ref())
        .and_then(|project_id| projects.iter().find(|project| &project.id == project_id));
    let project_label = project.map_or_else(
        || "no project".to_string(),
        |project| {
            format!(
                "{} ({})",
                display_project_name(&project.canonical_path),
                project.canonical_path
            )
        },
    );
    let is_chain = members.len() >= 2;
    let href = if is_chain {
        format!("/chains/{}", root.id)
    } else {
        conversation_href(current)
    };
    let title = item_title(&root, current, members.len());
    let task = task_status.as_ref().map_or_else(
        || "no readable task status".to_string(),
        |task_status| format!("task {task_status}"),
    );
    Ok(ResolveGlobalReferenceResponse {
        kind: "work".to_string(),
        id: id.to_string(),
        href: Some(href),
        title: Some(title),
        summary: format!(
            "{status} work item in {project_label}; root @conv:{}; current/latest @conv:{}; {task}",
            root.id, current.id
        ),
    })
}

fn map_db_not_found(e: DbError) -> AppError {
    match e {
        DbError::ConversationNotFound(_) => {
            AppError::NotFound("reference target not found".to_string())
        }
        other @ (DbError::Sqlx(_)
        | DbError::MessageNotFound(_)
        | DbError::SlugExists(_)
        | DbError::ConversationAlreadyExists(_)
        | DbError::Serialization(_)
        | DbError::ForkProposalConflict(_)) => AppError::Internal(other.to_string()),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn parse_session_row(row: sqlx::sqlite::SqliteRow) -> Result<GlobalRecallSession, sqlx::Error> {
    Ok(GlobalRecallSession {
        id: row.try_get("id")?,
        title: row.try_get("title")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?, "created_at")?,
        updated_at: parse_datetime(&row.try_get::<String, _>("updated_at")?, "updated_at")?,
    })
}

#[allow(clippy::needless_pass_by_value)]
fn parse_message_with_ordinal_row(
    row: sqlx::sqlite::SqliteRow,
) -> Result<(i64, GlobalRecallMessage), sqlx::Error> {
    let ordinal = row.try_get("ordinal")?;
    Ok((ordinal, parse_message_row(row)?))
}

#[allow(clippy::needless_pass_by_value)]
fn parse_message_row(row: sqlx::sqlite::SqliteRow) -> Result<GlobalRecallMessage, sqlx::Error> {
    Ok(GlobalRecallMessage {
        id: row.try_get("id")?,
        role: row.try_get("role")?,
        content: row.try_get("content")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?, "created_at")?,
    })
}

fn parse_datetime(s: &str, column: &'static str) -> Result<DateTime<Utc>, sqlx::Error> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: column.to_string(),
            source: Box::new(e),
        })
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

#[cfg(test)]
mod tests {
    use super::{
        format_open_work_for_agent, group_conversation_chains, is_intrinsically_closed,
        is_open_work_candidate, message_id_fragment, paginate_open_work, parse_conv_handle,
        split_fragment, GlobalOpenWorkItem, GlobalOpenWorkProject, GlobalOpenWorkResponse,
        GlobalOpenWorkSource,
    };
    use crate::db::{ConvMode, Conversation, NonEmptyString};
    use crate::state_machine::ConvState;
    use chrono::{Duration, Utc};
    use phoenix_core::llm_language::LlmLanguage;
    use sqlx::Row;

    fn conversation(id: &str) -> Conversation {
        let now = Utc::now();
        Conversation {
            id: id.to_string(),
            slug: Some(id.to_string()),
            title: Some(id.to_string()),
            cwd: "/tmp/project".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: None,
            project_id: Some("project-1".to_string()),
            kind: crate::db::ConversationKind::Standard,
            conv_mode: ConvMode::Direct,
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            llm_language: LlmLanguage::default(),
            spawned_from_conversation_id: None,
            transcript_generation: 1,
        }
    }

    fn work_mode() -> ConvMode {
        ConvMode::Work {
            branch_name: NonEmptyString::new("task-1").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/worktree").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("1").unwrap(),
            task_title: NonEmptyString::new("Test task").unwrap(),
        }
    }

    #[test]
    fn open_work_pages_preserve_project_groups() {
        let now = Utc::now();
        let item = |id: &str| GlobalOpenWorkItem {
            id: id.to_string(),
            source: GlobalOpenWorkSource::Conversation,
            title: id.to_string(),
            project_id: Some("project-1".to_string()),
            current_conversation_id: id.to_string(),
            current_conversation_slug: Some(id.to_string()),
            root_conversation_id: id.to_string(),
            root_conversation_slug: Some(id.to_string()),
            updated_at: now,
            mode: "Direct".to_string(),
            state: "Idle".to_string(),
            task_id: None,
            task_title: None,
            task_status: None,
            branch_name: None,
            base_branch: None,
            worktree_path: None,
            member_count: 1,
            signals: vec!["recent activity".to_string()],
            href: format!("/c/{id}"),
            reference: format!("@work:{id}"),
        };
        let view = GlobalOpenWorkResponse {
            generated_at: now,
            groups: vec![GlobalOpenWorkProject {
                project_id: Some("project-1".to_string()),
                project_name: "project".to_string(),
                canonical_path: Some("/tmp/project".to_string()),
                items: vec![item("a"), item("b"), item("c")],
            }],
            has_more: false,
        };

        let first = paginate_open_work(view, 0, 2);
        assert!(first.has_more);
        assert_eq!(first.groups[0].items.len(), 2);
        assert_eq!(first.groups[0].items[0].id, "a");
        let output = format_open_work_for_agent(&first, 0);
        assert!(output.contains("has_more: true"));
        assert!(output.contains("next_offset: 2"));
    }

    #[test]
    fn empty_open_work_tool_page_is_explicit() {
        let page = GlobalOpenWorkResponse {
            generated_at: Utc::now(),
            groups: vec![],
            has_more: false,
        };

        assert_eq!(
            format_open_work_for_agent(&page, 100),
            "Open work page offset 100\nhas_more: false\nnext_offset: none\nNo active work found.\n"
        );
    }

    #[tokio::test]
    async fn multiple_sessions_keep_ordered_independent_turns() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let now = Utc::now().to_rfc3339();
        for id in ["session-a", "session-b"] {
            sqlx::query(
                "INSERT INTO global_recall_sessions (id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
            )
            .bind(id)
            .bind(id)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        }

        super::insert_recall_turn(&db, "session-a", "question 1", "answer 1")
            .await
            .unwrap();
        super::insert_recall_turn(&db, "session-b", "question b", "answer b")
            .await
            .unwrap();
        super::insert_recall_turn(&db, "session-a", "question 2", "answer 2")
            .await
            .unwrap();

        let rows = sqlx::query(
            "SELECT ordinal, role, content FROM global_recall_messages
             WHERE session_id = 'session-a' ORDER BY ordinal",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        let values: Vec<(i64, String, String)> = rows
            .iter()
            .map(|row| (row.get("ordinal"), row.get("role"), row.get("content")))
            .collect();
        assert_eq!(
            values,
            vec![
                (0, "user".to_string(), "question 1".to_string()),
                (1, "assistant".to_string(), "answer 1".to_string()),
                (2, "user".to_string(), "question 2".to_string()),
                (3, "assistant".to_string(), "answer 2".to_string()),
            ]
        );
        let other_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM global_recall_messages WHERE session_id = 'session-b'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(other_count, 2);
    }

    #[test]
    fn open_work_requires_positive_evidence() {
        let now = Utc::now();
        let mut direct = conversation("direct");
        assert!(is_open_work_candidate(&direct, None, now));
        direct.updated_at = now - Duration::days(15);
        assert!(!is_open_work_candidate(&direct, None, now));

        let mut work = conversation("work");
        work.conv_mode = work_mode();
        assert!(!is_open_work_candidate(&work, None, now));
        assert!(is_open_work_candidate(&work, Some("ready"), now));
        assert!(is_open_work_candidate(&work, Some("in-progress"), now));
        assert!(is_open_work_candidate(&work, Some("blocked"), now));
        assert!(!is_open_work_candidate(&work, Some("done"), now));

        work.state = ConvState::AwaitingContinuation {
            rejected_tool_calls: vec![],
            attempt: 0,
        };
        assert!(is_open_work_candidate(&work, None, now));
        assert!(!is_open_work_candidate(&work, Some("wont-do"), now));

        direct.state = ConvState::Completed {
            result: "done".to_string(),
        };
        direct.updated_at = now;
        assert!(!is_open_work_candidate(&direct, None, now));
    }

    #[test]
    fn failed_and_cancelled_creation_are_not_open_work() {
        let now = Utc::now();
        let mut direct = conversation("direct");
        direct.state = ConvState::CreationFailed {
            job_id: "job-1".to_string(),
            error: "setup failed".to_string(),
            error_kind: phoenix_core::domain::db_schema::ErrorKind::ServerError,
        };
        assert!(!is_open_work_candidate(&direct, None, now));
        direct.state = ConvState::CreationCancelled {
            job_id: "job-1".to_string(),
        };
        assert!(!is_open_work_candidate(&direct, None, now));
    }

    #[test]
    fn intrinsic_open_work_exclusions_need_no_task_status() {
        let mut conv = conversation("closed");
        conv.conv_mode = work_mode();
        conv.archived = true;
        assert!(is_intrinsically_closed(&conv));

        conv.archived = false;
        conv.user_initiated = false;
        assert!(is_intrinsically_closed(&conv));

        conv.user_initiated = true;
        conv.state = ConvState::Terminal;
        assert!(is_intrinsically_closed(&conv));
    }

    #[test]
    fn archived_historical_members_do_not_break_chain_identity() {
        let mut root = conversation("root");
        root.archived = true;
        root.continued_in_conv_id = Some("middle".to_string());
        let mut middle = conversation("middle");
        middle.archived = true;
        middle.continued_in_conv_id = Some("current".to_string());
        let current = conversation("current");

        let chains = group_conversation_chains(&[current, middle, root]);
        assert_eq!(chains.len(), 1);
        let ids: Vec<&str> = chains[0].iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "middle", "current"]);
    }

    #[test]
    fn parse_conv_handle_accepts_message_handle_syntax() {
        assert_eq!(
            parse_conv_handle("conv-1 msg:message-9"),
            ("conv-1", Some("message-9"))
        );
        assert_eq!(
            parse_conv_handle("conv-1 msg:message-9:"),
            ("conv-1", Some("message-9"))
        );
        assert_eq!(
            parse_conv_handle("conv-1 msg: message-9"),
            ("conv-1", Some("message-9"))
        );
    }

    #[test]
    fn parse_conv_handle_preserves_message_fragment() {
        assert_eq!(
            parse_conv_handle("conv-1#message-message-9"),
            ("conv-1", Some("message-9"))
        );
    }

    #[test]
    fn split_fragment_keeps_base_and_message_id() {
        assert_eq!(
            split_fragment("slug#message-m1"),
            ("slug", Some("message-m1"))
        );
        assert_eq!(message_id_fragment("message-m1"), Some("m1"));
        assert_eq!(message_id_fragment("section-m1"), None);
    }
}
