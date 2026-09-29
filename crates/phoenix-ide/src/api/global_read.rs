use super::AppState;
use crate::db::MessageContent;
use crate::db::{Conversation, DbError, MessageType, RetrievalRequest, RetrievalScope};
use axum::{extract::State, Json};
use phoenix_llm::ContentBlock;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::sync::Arc;

use super::handlers::AppError;

const SEARCH_TOP_K: usize = 10;
const READ_PAGE_CHARS: usize = 7000;
const READ_MESSAGE_BATCH: i64 = 64;
const READ_TARGET_SIDE_MESSAGES: i64 = 32;
#[derive(Debug, PartialEq, Eq)]
struct ConversationReadTarget {
    conversation_id: String,
    message_id: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlobalMessageTarget {
    StableProductConversation { product_conversation_id: String },
    ExactTranscript { conversation_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlobalMessageTargetError {
    MissingId,
    UnsupportedSyntax,
    CoordinatorChainRejected,
    ConversationNotFound(String),
    SubAgentRejected,
    ResolutionFailed(String),
}

impl GlobalMessageTargetError {
    #[must_use]
    pub(crate) fn stable_code(&self) -> &'static str {
        match self {
            Self::MissingId => "missing_target_id",
            Self::UnsupportedSyntax => "unsupported_target_syntax",
            Self::CoordinatorChainRejected => "coordinator_chain_rejected",
            Self::ConversationNotFound(_) => "target_not_found",
            Self::SubAgentRejected => "sub_agent_target_rejected",
            Self::ResolutionFailed(_) => "target_resolution_failed",
        }
    }
}

impl std::fmt::Display for GlobalMessageTargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingId => write!(f, "message target is missing an id"),
            Self::UnsupportedSyntax => write!(f, "unsupported message target syntax"),
            Self::CoordinatorChainRejected => {
                write!(f, "the Coordinator chain cannot receive cross-conversation messages")
            }
            Self::ConversationNotFound(id) => write!(f, "conversation not found: {id}"),
            Self::SubAgentRejected => write!(
                f,
                "a sub-agent conversation cannot receive cross-conversation messages; message its parent conversation instead"
            ),
            Self::ResolutionFailed(message) => write!(f, "target resolution failed: {message}"),
        }
    }
}

impl std::error::Error for GlobalMessageTargetError {}

#[derive(Clone)]
pub(crate) struct GlobalReadService {
    db: crate::db::Database,
    message_retriever: Arc<dyn crate::db::MessageRetriever>,
}

#[derive(Debug)]
pub(crate) struct ValidatedCoordinatorBashSpawnTarget {
    pub(crate) path: std::path::PathBuf,
    pub(crate) work_scope_id: phoenix_core::work_scope::WorkScopeId,
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

    pub(crate) async fn resolve_active_work_scope_bash_target(
        &self,
        requested_work_scope_id: &str,
    ) -> Result<ValidatedCoordinatorBashSpawnTarget, String> {
        let row = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT environment.id, environment.worktree_path, environment.cwd
             FROM work_scopes environment
             WHERE environment.id = ?1
               AND environment.lifecycle = 'active'
               AND environment.environment_kind <> 'none'
               AND EXISTS (
                   SELECT 1
                   FROM conversations owner
                   LEFT JOIN product_conversations product
                     ON product.id = owner.product_conversation_id
                   WHERE owner.work_scope_id = environment.id
                     AND (
                         (product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open')
                         OR (COALESCE(product.kind, '') <> 'ordinary' AND owner.archived = 0)
                     )
                     AND json_extract(owner.state, '$.type') NOT IN (
                         'completed', 'failed', 'handed_off', 'creation_failed',
                         'creation_cancelled', 'terminal'
                     )
                     AND NOT (
                         json_extract(owner.state, '$.type') = 'context_exhausted'
                         AND owner.continued_in_conv_id IS NOT NULL
                     )
               )",
        )
        .bind(requested_work_scope_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|error| format!("failed to resolve Coordinator bash WorkScope: {error}"))?
        .ok_or_else(|| {
            "active persisted WorkScope with a live owner not found for Coordinator bash run"
                .to_string()
        })?;
        let (work_scope_id, worktree_path, cwd) = row;
        let preferred = worktree_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .or_else(|| cwd.as_deref().filter(|path| !path.trim().is_empty()))
            .ok_or_else(|| {
                "active persisted WorkScope is missing both worktree_path and cwd".to_string()
            })?;
        let canonical = crate::conversation_cwd::validate_conversation_cwd(preferred)
            .map_err(|error| format!("invalid persisted Coordinator bash cwd: {error}"))?
            .path_buf();
        Ok(ValidatedCoordinatorBashSpawnTarget {
            path: canonical,
            work_scope_id: phoenix_core::work_scope::WorkScopeId::parse(work_scope_id)
                .map_err(|error| format!("invalid persisted WorkScope id: {error}"))?,
        })
    }

    pub(crate) async fn query_database(
        &self,
        sql: &str,
    ) -> Result<phoenix_db::CoordinatorQueryResult, String> {
        self.db
            .coordinator_query(sql)
            .await
            .map_err(|error| error.to_string())
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
        let coordinator_chain = self.coordinator_chain_ids().await?;
        let hits = self
            .message_retriever
            .retrieve(RetrievalRequest::natural_language(
                query,
                RetrievalScope::GlobalExcluding(coordinator_chain),
                SEARCH_TOP_K,
            ))
            .await
            .map_err(|e| format!("search failed: {e}"))?;
        if hits.is_empty() {
            Ok("No matching messages found.".to_string())
        } else {
            Ok(format_global_search_hits(self, &hits).await)
        }
    }

    async fn coordinator_chain_ids(&self) -> Result<Vec<String>, String> {
        let Some(coordinator_id) = self
            .db
            .coordinator_conversation_id()
            .await
            .map_err(|e| format!("failed to resolve Coordinator: {e}"))?
        else {
            return Ok(Vec::new());
        };
        let root_id = self
            .db
            .chain_root_of(&coordinator_id)
            .await
            .map_err(|e| format!("failed to resolve Coordinator chain: {e}"))?
            .unwrap_or(coordinator_id);
        self.db
            .chain_members_forward(&root_id)
            .await
            .map_err(|e| format!("failed to read Coordinator chain: {e}"))
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

    pub(crate) async fn product_conversation_id_for_transcript(
        &self,
        conversation_id: &str,
    ) -> Result<String, String> {
        self.db
            .get_conversation(conversation_id)
            .await
            .map(|conversation| conversation.product_conversation_id.to_string())
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn resolve_message_target(
        &self,
        target: &str,
    ) -> Result<GlobalMessageTarget, GlobalMessageTargetError> {
        resolve_global_message_target(self, target).await
    }

    pub(crate) async fn resolve_reference(
        &self,
        reference: &str,
    ) -> Result<ResolveGlobalReferenceResponse, AppError> {
        resolve_reference_impl(self, reference).await
    }
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

async fn resolve_conversation_read_target(
    service: &GlobalReadService,
    raw: &str,
) -> Result<ConversationReadTarget, String> {
    let reference = raw.trim();
    if let Some(id) = reference.strip_prefix("@conv:") {
        if id.is_empty() || id.contains('#') {
            return Err(
                "ProductConversation reference must be @conv:<product_conversation_id>".to_string(),
            );
        }
        let product_conversation_id =
            phoenix_core::domain::product_conversation::ProductConversationId::parse(id)
                .expect("non-empty typed reference");
        let aggregate = service
            .db
            .get_ordinary_product_conversation(&product_conversation_id)
            .await
            .map_err(|_| "ProductConversation reference not found".to_string())?;
        return Ok(ConversationReadTarget {
            conversation_id: aggregate.latest_transcript_row_id,
            message_id: None,
        });
    }
    if let Some(rest) = reference.strip_prefix("@transcript:") {
        let (id, message_id) = parse_conv_handle(rest);
        if id.is_empty() {
            return Err("transcript reference is missing an id".to_string());
        }
        return Ok(ConversationReadTarget {
            conversation_id: id.to_string(),
            message_id: message_id.map(str::to_string),
        });
    }
    Err(
        "conversation must be @conv:<product_conversation_id> or @transcript:<conversation_id>"
            .to_string(),
    )
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
        let link = href.unwrap_or_else(|| {
            format!(
                "@transcript:{}#message-{}",
                hit.conversation_id, hit.message_id
            )
        });
        let _ = writeln!(
            out,
            "- [{} · {} · {}]({}) @transcript:{}#message-{} — {}",
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
        "Transcript @transcript:{} — {}\nlink: {}\nupdated: {}\n---\n",
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
        "Transcript @transcript:{} — {}\nlink: {}\nupdated: {}\ntarget_message: {}\nhas_more_before: {}\nhas_more_after: {}\n---\n",
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
        MessageType::User | MessageType::Agent | MessageType::Skill | MessageType::Continuation
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
        "[{} · {} · {}]({}) @transcript:{}#message-{}\n{}\n\n",
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
                    "read_conversation: dropping user-message images — image recall is unsupported",
                );
                let _ = write!(
                    text,
                    "\n[{} image(s) attached to this message are not shown — read_conversation returns text only]",
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
                    "read_conversation: dropping tool-result images — image recall is unsupported",
                );
                format!(
                    "{}\n[{} image(s) in this tool result are not shown — read_conversation returns text only]",
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

async fn resolve_reference_impl(
    service: &GlobalReadService,
    raw: &str,
) -> Result<ResolveGlobalReferenceResponse, AppError> {
    let reference = raw.trim();
    if let Some(rest) = reference
        .strip_prefix("/c/")
        .or_else(|| reference.strip_prefix("/global/"))
    {
        let (slug, fragment) = split_fragment(rest);
        let conv = load_conversation_by_slug_or_id(service, slug).await?;
        if let Some(message_id) = fragment.and_then(message_id_fragment) {
            return resolve_message(service, conv, message_id, true).await;
        }
        return Ok(resolve_conversation(conv, true));
    }
    if let Some(rest) = reference.strip_prefix("/chains/") {
        let (id, _) = split_fragment(rest);
        return resolve_chain(service, id).await;
    }
    if let Some(id) = reference.strip_prefix("@conv:") {
        if id.is_empty() || id.contains('#') {
            return Err(AppError::BadRequest(
                "ProductConversation reference must be @conv:<product_conversation_id>".to_string(),
            ));
        }
        let typed_id = phoenix_core::domain::product_conversation::ProductConversationId::parse(id)
            .expect("non-empty typed reference");
        let aggregate = service
            .db
            .get_ordinary_product_conversation(&typed_id)
            .await
            .map_err(map_db_not_found)?;
        let current = service
            .db
            .get_conversation(&aggregate.latest_transcript_row_id)
            .await
            .map_err(map_db_not_found)?;
        return Ok(ResolveGlobalReferenceResponse {
            kind: "product_conversation".to_string(),
            id: typed_id.to_string(),
            href: Some(format!("/product-conversations/{typed_id}")),
            title: current.title.clone().or(current.slug.clone()),
            summary: format!(
                "stable ProductConversation @conv:{typed_id}; current transcript @transcript:{}; state {}; updated {}",
                current.id,
                current.state.variant_name(),
                current.updated_at.to_rfc3339()
            ),
        });
    }
    if let Some(rest) = reference.strip_prefix("@transcript:") {
        let (id, message_id) = parse_conv_handle(rest);
        let conv = service
            .db
            .get_conversation(id)
            .await
            .map_err(map_db_not_found)?;
        if let Some(message_id) = message_id {
            return resolve_message(service, conv, message_id, false).await;
        }
        return Ok(resolve_conversation(conv, false));
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

async fn resolve_global_message_target(
    service: &GlobalReadService,
    raw: &str,
) -> Result<GlobalMessageTarget, GlobalMessageTargetError> {
    let reference = raw.trim();
    if let Some(product_conversation_id) = reference.strip_prefix("@conv:") {
        if product_conversation_id.is_empty() {
            return Err(GlobalMessageTargetError::MissingId);
        }
        if product_conversation_id.contains('#') {
            return Err(GlobalMessageTargetError::UnsupportedSyntax);
        }
        let typed_id = phoenix_core::domain::product_conversation::ProductConversationId::parse(
            product_conversation_id,
        )
        .expect("non-empty typed reference");
        service
            .db
            .get_ordinary_product_conversation(&typed_id)
            .await
            .map_err(|_| {
                GlobalMessageTargetError::ConversationNotFound(product_conversation_id.to_string())
            })?;
        return Ok(GlobalMessageTarget::StableProductConversation {
            product_conversation_id: product_conversation_id.to_string(),
        });
    }
    let Some(conversation_id) = reference.strip_prefix("@transcript:") else {
        return Err(if reference.is_empty() {
            GlobalMessageTargetError::MissingId
        } else {
            GlobalMessageTargetError::UnsupportedSyntax
        });
    };
    if conversation_id.is_empty() {
        return Err(GlobalMessageTargetError::MissingId);
    }
    if conversation_id.contains('#') {
        return Err(GlobalMessageTargetError::UnsupportedSyntax);
    }
    let conversation = service
        .db
        .get_conversation(conversation_id)
        .await
        .map_err(|_| GlobalMessageTargetError::ConversationNotFound(conversation_id.to_string()))?;
    if conversation.parent_conversation_id.is_some() {
        return Err(GlobalMessageTargetError::SubAgentRejected);
    }
    if service
        .coordinator_chain_ids()
        .await
        .map_err(GlobalMessageTargetError::ResolutionFailed)?
        .contains(&conversation.id)
    {
        return Err(GlobalMessageTargetError::CoordinatorChainRejected);
    }
    Ok(GlobalMessageTarget::ExactTranscript {
        conversation_id: conversation.id,
    })
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
    let uuid_shaped = uuid::Uuid::parse_str(slug_or_id).is_ok();
    if uuid_shaped {
        match service.db.get_conversation(slug_or_id).await {
            Ok(conv) => return Ok(conv),
            Err(DbError::ConversationNotFound(_)) => {}
            Err(e) => return Err(map_db_not_found(e)),
        }
    }
    match service.db.get_conversation_by_slug(slug_or_id).await {
        Ok(conv) => Ok(conv),
        Err(DbError::ConversationNotFound(_)) if !uuid_shaped => service
            .db
            .get_conversation(slug_or_id)
            .await
            .map_err(map_db_not_found),
        Err(e) => Err(map_db_not_found(e)),
    }
}

fn resolve_conversation(conv: Conversation, global_href: bool) -> ResolveGlobalReferenceResponse {
    let href = Some(if global_href {
        format!("/global/{}", conv.id)
    } else {
        conversation_href(&conv)
    });
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
    global_href: bool,
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
    let href = Some(if global_href {
        format!("/global/{}#message-{}", conv.id, message.message_id)
    } else {
        conversation_message_href(&conv, Some((&message.message_id, message.message_type)))
    });
    let title = conv.title.clone().or(conv.slug.clone());
    Ok(ResolveGlobalReferenceResponse {
        kind: "message".to_string(),
        id: message.message_id.clone(),
        href,
        title,
        summary: format!(
            "{} message {} in @transcript:{} at {}: {}",
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
        .map(|member| format!("@transcript:{member}"))
        .collect::<Vec<_>>()
        .join(", ");
    let current = members.last().map_or_else(
        || format!("@transcript:{root_id}"),
        |member| format!("@transcript:{member}"),
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
            "legacy chain rooted at @transcript:{root_id} with {} member(s); current/latest {}; ordered members: {}",
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
    let is_chain = members.len() >= 2;
    let href = if is_chain {
        format!("/chains/{}", root.id)
    } else {
        conversation_href(current)
    };
    let title = root
        .chain_name
        .clone()
        .or(root.title.clone())
        .or(current.title.clone())
        .or(current.slug.clone())
        .unwrap_or_else(|| current.id.clone());
    Ok(ResolveGlobalReferenceResponse {
        kind: "work".to_string(),
        id: id.to_string(),
        href: Some(href),
        title: Some(title),
        summary: format!(
            "work reference identity; root @transcript:{}; current/latest @transcript:{}; current state {}; state updated {}; conversation updated {}; archived {}",
            root.id,
            current.id,
            current.state.variant_name(),
            current.state_updated_at,
            current.updated_at,
            current.archived
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
        | DbError::MessageConflict(_)
        | DbError::SlugExists(_)
        | DbError::ConversationAlreadyExists(_)
        | DbError::Serialization(_)
        | DbError::SubAgentLifecycleConflict(_)
        | DbError::ContinuationPrecondition(_)
        | DbError::CloseFoundationConflict(_)
        | DbError::CloseAdmissionFenced(_)
        | DbError::ProductConversationUnavailable(_)
        | DbError::SteeringQueueFull
        | DbError::CloseFoundationPrecondition(_)
        | DbError::CloseFoundationStaleLatest { .. }
        | DbError::CloseFoundationRepairRequired(_)
        | DbError::CloseFoundationNotFound(_)
        | DbError::ForkProposalConflict(_)
        | DbError::DirectTurnConflict(_)
        | DbError::GitRepositoryWorkScopeProjectConflict { .. }
        | DbError::DormantGitRepositoryCatchupPermitTargetMismatch
        | DbError::DormantGitRepositoryCatchupStaleOperation
        | DbError::DormantGitRepositoryCatchupBlockedByReadinessClaim
        | DbError::DormantGitRepositoryReadinessCatchupInProgress
        | DbError::DormantGitRepositoryReadinessReceiptTargetMismatch
        | DbError::DormantGitRepositoryReadinessReceiptOperationMismatch) => {
            AppError::Internal(other.to_string())
        }
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
        message_id_fragment, parse_conv_handle, render_full_message_text,
        resolve_conversation_read_target, split_fragment, GlobalMessageTarget,
        GlobalMessageTargetError, GlobalReadService,
    };
    use std::sync::Arc;

    #[test]
    fn message_target_rejections_are_caller_neutral() {
        assert_eq!(
            GlobalMessageTargetError::CoordinatorChainRejected.to_string(),
            "the Coordinator chain cannot receive cross-conversation messages"
        );
        assert_eq!(
            GlobalMessageTargetError::SubAgentRejected.to_string(),
            "a sub-agent conversation cannot receive cross-conversation messages; message its parent conversation instead"
        );
    }

    #[test]
    fn transcript_image_placeholder_is_caller_neutral() {
        let mut content = phoenix_core::domain::db_schema::UserContent::new("text");
        content
            .images
            .push(phoenix_core::domain::db_schema::ImageData {
                data: "encoded".to_string(),
                media_type: "image/png".to_string(),
            });
        let message = crate::db::Message {
            message_id: "message".to_string(),
            conversation_id: "conversation".to_string(),
            sequence_id: 1,
            message_type: phoenix_core::domain::db_schema::MessageType::User,
            content: phoenix_core::domain::db_schema::MessageContent::User(content),
            display_data: None,
            usage_data: None,
            created_at: chrono::Utc::now(),
        };

        let rendered = render_full_message_text(&message);

        assert!(rendered.contains("read_conversation returns text only"));
        assert!(!rendered.contains("Coordinator reads text only"));
    }

    #[test]
    fn parses_durable_conversation_references() {
        assert_eq!(
            split_fragment("/c/slug#message-id"),
            ("/c/slug", Some("message-id"))
        );
        assert_eq!(message_id_fragment("message-id"), Some("id"));
        assert_eq!(parse_conv_handle("abc#message-def"), ("abc", Some("def")));
    }

    #[tokio::test]
    async fn stable_product_reference_reads_current_transcript_and_exact_reference_stays_pinned() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        let root = db.get_conversation("root").await.unwrap();
        let product_id = root.product_conversation_id;
        sqlx::query("UPDATE conversations SET state = '{\"type\":\"context_exhausted\",\"summary\":\"continue\"}', state_kind = 'context_exhausted' WHERE id = 'root'")
            .execute(db.pool())
            .await
            .unwrap();
        let current = match db.continue_conversation("root").await.unwrap() {
            crate::db::ContinueOutcome::Created(current) => current,
            crate::db::ContinueOutcome::AlreadyContinued(current) => {
                panic!("unexpected existing continuation: {current:?}")
            }
            crate::db::ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("unexpected parent state: {state_variant}")
            }
        };
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));

        let stable =
            resolve_conversation_read_target(&service, &format!("@conv:{}", product_id.as_str()))
                .await
                .unwrap();
        let exact = resolve_conversation_read_target(&service, "@transcript:root")
            .await
            .unwrap();

        assert_eq!(stable.conversation_id, current.id);
        assert_eq!(exact.conversation_id, "root");
        assert!(resolve_conversation_read_target(&service, "root")
            .await
            .unwrap_err()
            .contains("must be @conv"));
    }

    #[tokio::test]
    async fn message_targets_accept_only_typed_stable_or_exact_references() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        let root = db.get_conversation("root").await.unwrap();
        let product_id = root.product_conversation_id;
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));

        assert_eq!(
            service
                .resolve_message_target(&format!("@conv:{}", product_id.as_str()))
                .await
                .unwrap(),
            GlobalMessageTarget::StableProductConversation {
                product_conversation_id: product_id.to_string(),
            }
        );
        assert_eq!(
            service
                .resolve_message_target("@transcript:root")
                .await
                .unwrap(),
            GlobalMessageTarget::ExactTranscript {
                conversation_id: "root".to_string(),
            }
        );
        for rejected in ["root", "/c/root", "@work:root", "@conv:root#message-id"] {
            assert!(
                service.resolve_message_target(rejected).await.is_err(),
                "{rejected}"
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn coordinator_bash_scope_resolution_prefers_nonempty_worktree_path_then_cwd_and_canonicalizes(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cwd-resolution.db");
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let canonical = work.canonicalize().unwrap();
        let db = crate::db::Database::open(path.to_str().unwrap())
            .await
            .unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation(
            "scope-owner",
            "scope-owner",
            canonical.to_str().unwrap(),
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let retriever = Arc::new(db.fts_retriever());
        let service = GlobalReadService::new(db.clone(), retriever);
        let work_scope_id = db
            .get_conversation("scope-owner")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();

        let binding = service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .unwrap();
        assert_eq!(binding.path, canonical);

        let fallback = dir.path().join("fallback");
        std::fs::create_dir(&fallback).unwrap();
        let fallback_noncanonical = fallback.join("..").join("fallback");
        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'unowned_cwd', worktree_path = NULL,
                 branch_name = NULL, base_branch = NULL, cwd = ?1
             WHERE id = ?2",
        )
        .bind(fallback_noncanonical.to_str().unwrap())
        .bind(work_scope_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        let binding = service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .unwrap();
        assert_eq!(binding.path, fallback.canonicalize().unwrap());

        sqlx::query(
            "UPDATE conversations
             SET state = '{\"type\":\"context_exhausted\",\"summary\":\"continue me\"}',
                 state_kind = 'context_exhausted'
             WHERE id = 'scope-owner'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .is_ok());
        sqlx::query(
            "UPDATE conversations
             SET state = '{\"type\":\"idle\"}', state_kind = 'idle'
             WHERE id = 'scope-owner'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'scope-owner'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .is_ok());
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history'
             WHERE id = (SELECT product_conversation_id FROM conversations WHERE id = 'scope-owner')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .unwrap_err()
            .contains("live owner not found"));
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'open'
             WHERE id = (SELECT product_conversation_id FROM conversations WHERE id = 'scope-owner')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "UPDATE work_scopes
             SET lifecycle = 'retired', retired_at = '2026-01-01T00:00:00Z',
                 environment_kind = 'allocated_worktree', worktree_path = ?1,
                 branch_name = 'feature/history', base_branch = 'main'
             WHERE id = ?2",
        )
        .bind(canonical.to_str().unwrap())
        .bind(work_scope_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        assert!(service
            .resolve_active_work_scope_bash_target(work_scope_id.as_str())
            .await
            .unwrap_err()
            .contains("active persisted WorkScope with a live owner not found"));
    }
}
