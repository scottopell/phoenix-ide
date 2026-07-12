use super::AppState;
use crate::db::MessageContent;
use crate::db::{ConvMode, Conversation, DbError, MessageType, RetrievalScope};
use crate::state_machine::ConvState;
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use phoenix_llm::ContentBlock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use super::handlers::AppError;

const RECENT_DAYS: i64 = 14;
const SEARCH_TOP_K: usize = 10;
const READ_PAGE_CHARS: usize = 7000;
const READ_MESSAGE_BATCH: i64 = 64;
const READ_TARGET_SIDE_MESSAGES: i64 = 32;
const OPEN_WORK_PAGE: usize = 100;

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

#[derive(Clone)]
pub(crate) struct GlobalReadService {
    db: crate::db::Database,
    message_retriever: Arc<dyn crate::db::MessageRetriever>,
}

impl GlobalReadService {
    pub(crate) fn new(
        db: crate::db::Database,
        message_retriever: Arc<dyn crate::db::MessageRetriever>,
    ) -> Self {
        Self {
            db,
            message_retriever,
        }
    }

    fn from_state(state: &AppState) -> Self {
        Self::new(state.db.clone(), state.message_retriever.clone())
    }

    pub(crate) async fn open_work(&self) -> Result<GlobalOpenWorkResponse, AppError> {
        build_open_work(self).await
    }

    pub(crate) async fn search(&self, query: &str) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("query is required".to_string());
        }
        if !self.message_retriever.index_reconciled() {
            return Err(
                "the global message index is still warming; try again after startup reconciliation completes"
                    .to_string(),
            );
        }
        let hits = self
            .message_retriever
            .retrieve(query, RetrievalScope::Global, SEARCH_TOP_K)
            .await
            .map_err(|e| format!("search failed: {e}"))?;
        if hits.is_empty() {
            Ok("No matching messages found.".to_string())
        } else {
            Ok(format_global_search_hits(self, &hits).await)
        }
    }

    pub(crate) async fn read_conversation(
        &self,
        conversation: &str,
        cursor: usize,
    ) -> Result<String, String> {
        let target = resolve_conversation_read_target(self, conversation).await?;
        let conv = self
            .db
            .get_conversation(&target.conversation_id)
            .await
            .map_err(|e| format!("conversation not found: {e}"))?;
        if let Some(message_id) = target.message_id.as_deref() {
            read_conversation_around_message(&self.db, &conv, message_id)
                .await
                .map_err(|e| format!("read failed: {e}"))
        } else {
            read_conversation_page(&self.db, &conv, cursor)
                .await
                .map_err(|e| format!("read failed: {e}"))
        }
    }

    pub(crate) async fn open_work_page(&self, offset: usize) -> Result<String, String> {
        let view = self
            .open_work()
            .await
            .map_err(|e| format!("open work projection failed: {e:?}"))?;
        Ok(format_open_work_for_agent(
            &paginate_open_work(view, offset, OPEN_WORK_PAGE),
            offset,
        ))
    }

    pub(crate) async fn resolve_reference(
        &self,
        reference: &str,
    ) -> Result<ResolveGlobalReferenceResponse, AppError> {
        resolve_reference_impl(self, reference).await
    }
}

pub async fn open_work(
    State(state): State<AppState>,
    Query(page): Query<OpenWorkPageQuery>,
) -> Result<Json<GlobalOpenWorkResponse>, AppError> {
    let view = GlobalReadService::from_state(&state).open_work().await?;
    Ok(Json(paginate_open_work(view, page.offset, OPEN_WORK_PAGE)))
}

pub async fn resolve_reference(
    State(state): State<AppState>,
    Json(req): Json<ResolveGlobalReferenceRequest>,
) -> Result<Json<ResolveGlobalReferenceResponse>, AppError> {
    Ok(Json(
        GlobalReadService::from_state(&state)
            .resolve_reference(&req.reference)
            .await?,
    ))
}

async fn build_open_work(service: &GlobalReadService) -> Result<GlobalOpenWorkResponse, AppError> {
    let now = Utc::now();
    let conversations = service
        .db
        .list_all_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let coordinator_id = service
        .db
        .coordinator_conversation_id()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let conversations: Vec<_> = conversations
        .into_iter()
        .filter(|conversation| Some(&conversation.id) != coordinator_id.as_ref())
        .collect();
    let projects = service
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

async fn resolve_conversation_read_target(
    service: &GlobalReadService,
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
        let conv = load_conversation_by_slug_or_id(service, slug)
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

async fn format_global_search_hits(
    service: &GlobalReadService,
    hits: &[crate::db::RetrievedChunk],
) -> String {
    let mut out = String::new();
    for hit in hits {
        let (title, href) = match service.db.get_conversation(&hit.conversation_id).await {
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
                    "coordinator read_conversation: dropping user-message images — image recall is unsupported",
                );
                let _ = write!(
                    text,
                    "\n[{} image(s) attached to this message are not shown — Coordinator reads text only]",
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
                    "coordinator read_conversation: dropping tool-result images — image recall is unsupported",
                );
                format!(
                    "{}\n[{} image(s) in this tool result are not shown — Coordinator reads text only]",
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
    service: &GlobalReadService,
    raw: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let reference = raw.trim();
    if let Some(rest) = reference.strip_prefix("/c/") {
        let (slug, fragment) = split_fragment(rest);
        let conv = load_conversation_by_slug_or_id(service, slug).await?;
        if let Some(message_id) = fragment.and_then(message_id_fragment) {
            return resolve_message(service, conv, message_id).await;
        }
        return Ok(resolve_conversation(conv));
    }
    if let Some(rest) = reference.strip_prefix("/chains/") {
        let (id, _) = split_fragment(rest);
        return resolve_chain(service, id).await;
    }
    if let Some(rest) = reference.strip_prefix("@conv:") {
        let (id, message_id) = parse_conv_handle(rest);
        let conv = service
            .db
            .get_conversation(id)
            .await
            .map_err(map_db_not_found)?;
        if let Some(message_id) = message_id {
            return resolve_message(service, conv, message_id).await;
        }
        return Ok(resolve_conversation(conv));
    }
    if let Some(rest) = reference.strip_prefix("@chain:") {
        let (id, _) = split_fragment(rest);
        return resolve_chain(service, first_token(id)).await;
    }
    if let Some(rest) = reference.strip_prefix("@work:") {
        let (id, _) = split_fragment(rest);
        return resolve_work(service, first_token(id)).await;
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
    service: &GlobalReadService,
    slug_or_id: &str,
) -> Result<Conversation, AppError> {
    match service.db.get_conversation_by_slug(slug_or_id).await {
        Ok(conv) => Ok(conv),
        Err(DbError::ConversationNotFound(_)) => service
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
    service: &GlobalReadService,
    conv: Conversation,
    message_id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let message = service
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
    service: &GlobalReadService,
    root_id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let root = service
        .db
        .get_conversation(root_id)
        .await
        .map_err(map_db_not_found)?;
    let members = service
        .db
        .chain_members_forward(root_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let resolved_root = service
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
    service: &GlobalReadService,
    id: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let root = service
        .db
        .get_conversation(id)
        .await
        .map_err(map_db_not_found)?;
    let chain_root = service
        .db
        .chain_root_of(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if chain_root.as_deref().is_some_and(|root_id| root_id != id) {
        return Err(AppError::NotFound(
            "work reference target not found".to_string(),
        ));
    }
    let member_ids = service
        .db
        .chain_members_forward(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut members = Vec::with_capacity(member_ids.len().max(1));
    if member_ids.len() >= 2 {
        for member_id in member_ids {
            members.push(
                service
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
    let projects = service
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
