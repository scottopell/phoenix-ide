use super::AppState;
use crate::db::MessageContent;
use crate::db::{Conversation, DbError, MessageType, RetrievalRequest, RetrievalScope};
use axum::{extract::State, Json};
use phoenix_llm::ContentBlock;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::fmt::Write as _;
use std::sync::Arc;

use super::handlers::AppError;

const SEARCH_TOP_K: usize = 10;
const READ_PAGE_CHARS: usize = 7000;
const READ_MESSAGE_BATCH: i64 = 64;
const READ_TARGET_SIDE_MESSAGES: i64 = 32;
#[derive(Debug, PartialEq, Eq)]
enum ConversationReadTarget {
    StableCurrent {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
    },
    Exact {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
        message_id: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveGlobalReferenceRequest {
    pub reference: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveGlobalReferenceResponse {
    pub kind: String,
    pub id: String,
    pub href: Option<String>,
    pub title: Option<String>,
    pub work_scope: ResolvedWorkScope,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvedWorkScope {
    Missing,
    Unavailable {
        work_scope_id: phoenix_core::work_scope::WorkScopeId,
        reason: WorkScopeUnavailableReason,
    },
    Available {
        work_scope_id: phoenix_core::work_scope::WorkScopeId,
        lifecycle: phoenix_core::work_scope::WorkScopeLifecycle,
        environment_kind: ResolvedEnvironmentKind,
        cwd: Option<String>,
        worktree_path: Option<String>,
        effective_path: Option<String>,
        path_semantics: ServerPathSemantics,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkScopeUnavailableReason {
    RecordNotFound,
    InvalidRecord,
    ReadFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedEnvironmentKind {
    AllocatedWorktree,
    UnownedCwd,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerPathSemantics {
    ServerFilesystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlobalMessageTarget {
    StableProductConversation {
        product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
    },
    ExactTranscript {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CanonicalConversationReference {
    StableProductConversation(phoenix_core::domain::product_conversation::ProductConversationId),
    ExactTranscript {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
        message_id: Option<String>,
    },
}

fn parse_canonical_conversation_reference(
    raw: &str,
    allow_message_fragment: bool,
) -> Result<CanonicalConversationReference, GlobalMessageTargetError> {
    let reference = raw.trim();
    if reference != raw {
        return Err(GlobalMessageTargetError::UnsupportedSyntax);
    }
    if let Some(id) = reference.strip_prefix("@conv:") {
        if id.contains('#') || id.chars().any(char::is_whitespace) {
            return Err(GlobalMessageTargetError::UnsupportedSyntax);
        }
        let id = phoenix_core::domain::product_conversation::ProductConversationId::parse(id)
            .map_err(|_| GlobalMessageTargetError::MissingId)?;
        return Ok(CanonicalConversationReference::StableProductConversation(
            id,
        ));
    }
    let Some(rest) = reference.strip_prefix("@transcript:") else {
        return Err(if reference.is_empty() {
            GlobalMessageTargetError::MissingId
        } else {
            GlobalMessageTargetError::UnsupportedSyntax
        });
    };
    let (id, fragment) = split_fragment(rest);
    if rest.chars().any(char::is_whitespace) {
        return Err(GlobalMessageTargetError::UnsupportedSyntax);
    }
    let transcript_id = phoenix_core::domain::close::TranscriptConversationId::parse(id)
        .map_err(|_| GlobalMessageTargetError::MissingId)?;
    let message_id = match fragment {
        Some(fragment) if allow_message_fragment => Some(
            message_id_fragment(fragment)
                .ok_or(GlobalMessageTargetError::UnsupportedSyntax)?
                .to_string(),
        ),
        Some(_) => return Err(GlobalMessageTargetError::UnsupportedSyntax),
        None => None,
    };
    Ok(CanonicalConversationReference::ExactTranscript {
        transcript_id,
        message_id,
    })
}

#[derive(Clone)]
pub(crate) struct GlobalReadService {
    db: crate::db::Database,
    message_retriever: Arc<dyn crate::db::MessageRetriever>,
    #[cfg(test)]
    stable_resolution_test_hook: Option<Arc<StableResolutionTestHook>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedCoordinatorBashSpawnTarget {
    pub(crate) path: std::path::PathBuf,
    pub(crate) work_scope_id: phoenix_core::work_scope::WorkScopeId,
    pub(crate) owner_name: String,
    pub(crate) owner_product_conversation_id: Option<String>,
    pub(crate) project_path: Option<String>,
}

#[cfg(test)]
struct StableResolutionTestHook {
    snapshot_read: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinatorWorkScopeTargetError {
    Authority,
    Persistence,
    Read,
}

impl GlobalReadService {
    pub(crate) fn new(
        db: crate::db::Database,
        message_retriever: Arc<dyn crate::db::MessageRetriever>,
    ) -> Self {
        Self {
            db,
            message_retriever,
            #[cfg(test)]
            stable_resolution_test_hook: None,
        }
    }

    fn from_state(state: &AppState) -> Self {
        Self::new(state.db.clone(), state.message_retriever.clone())
    }

    #[cfg(test)]
    fn install_stable_resolution_test_hook(&mut self) -> Arc<StableResolutionTestHook> {
        let hook = Arc::new(StableResolutionTestHook {
            snapshot_read: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        self.stable_resolution_test_hook = Some(hook.clone());
        hook
    }

    pub(crate) async fn resolve_active_work_scope_bash_target(
        &self,
        requested_work_scope_id: &str,
    ) -> Result<ValidatedCoordinatorBashSpawnTarget, CoordinatorWorkScopeTargetError> {
        let row = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT environment.id, environment.worktree_path, environment.cwd
             FROM work_scopes environment
             WHERE environment.id = ?1
               AND environment.lifecycle = 'active'
               AND environment.environment_kind <> 'none'
               AND EXISTS (
                   SELECT 1 FROM conversations owner
                   LEFT JOIN product_conversations product ON product.id = owner.product_conversation_id
                   WHERE owner.work_scope_id = environment.id
                     AND ((product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open')
                       OR (COALESCE(product.kind, '') <> 'ordinary' AND owner.archived = 0))
                     AND owner.state_kind NOT IN (
                       'completed', 'failed', 'handed_off', 'creation_failed', 'creation_cancelled', 'terminal')
                     AND NOT (owner.state_kind = 'context_exhausted'
                       AND owner.continued_in_conv_id IS NOT NULL)
               )",
        )
        .bind(requested_work_scope_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| CoordinatorWorkScopeTargetError::Persistence)?
        .ok_or(CoordinatorWorkScopeTargetError::Authority)?;
        let (work_scope_id, worktree_path, cwd) = row;
        let (owner_name, owner_product_conversation_id, project_path) =
            sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT COALESCE(NULLIF(root.chain_name, ''), NULLIF(root.title, ''), NULLIF(root.slug, ''), 'Untitled conversation'), owner.product_conversation_id, project.canonical_path
             FROM conversations owner
             LEFT JOIN projects project ON project.id = owner.project_id
             LEFT JOIN product_conversations product ON product.id = owner.product_conversation_id
             LEFT JOIN conversations root ON root.product_conversation_id = product.id
               AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM conversations predecessor
                 WHERE predecessor.product_conversation_id = root.product_conversation_id
                   AND predecessor.continued_in_conv_id = root.id)
             WHERE owner.work_scope_id = ?1
               AND owner.parent_conversation_id IS NULL
               AND owner.continued_in_conv_id IS NULL
               AND ((product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open')
                 OR (COALESCE(product.kind, '') <> 'ordinary' AND owner.archived = 0))
               AND owner.state_kind NOT IN (
                 'completed', 'failed', 'handed_off', 'creation_failed', 'creation_cancelled', 'terminal')
             ORDER BY owner.created_at DESC LIMIT 1",
        )
        .bind(&work_scope_id)
        .fetch_one(self.db.pool())
        .await
        .map_err(|_| CoordinatorWorkScopeTargetError::Persistence)?;
        let preferred = worktree_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .or_else(|| cwd.as_deref().filter(|path| !path.trim().is_empty()))
            .ok_or(CoordinatorWorkScopeTargetError::Read)?;
        let canonical = crate::conversation_cwd::validate_conversation_cwd(preferred)
            .map_err(|_| CoordinatorWorkScopeTargetError::Read)?
            .path_buf();
        Ok(ValidatedCoordinatorBashSpawnTarget {
            path: canonical,
            work_scope_id: phoenix_core::work_scope::WorkScopeId::parse(work_scope_id)
                .map_err(|_| CoordinatorWorkScopeTargetError::Persistence)?,
            owner_name,
            owner_product_conversation_id,
            project_path,
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
        let hits = self.search_hits(query).await?;
        self.format_search_hits(&hits).await
    }

    pub(crate) async fn search_hits(
        &self,
        query: &str,
    ) -> Result<Vec<crate::db::RetrievedChunk>, String> {
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
            .retrieve(self.search_request(query).await?)
            .await
            .map_err(|e| format!("search failed: {e}"))?;
        Ok(hits)
    }

    pub(crate) async fn format_search_hits(
        &self,
        hits: &[crate::db::RetrievedChunk],
    ) -> Result<String, String> {
        if hits.is_empty() {
            Ok("No matching messages found.".to_string())
        } else {
            format_global_search_hits(self, hits)
                .await
                .map_err(|e| format!("search citation failed: {e}"))
        }
    }

    pub(crate) async fn search_request(&self, query: &str) -> Result<RetrievalRequest, String> {
        let coordinator_chain = self.coordinator_chain_ids().await?;
        Ok(RetrievalRequest::natural_language(
            query,
            RetrievalScope::GlobalExcluding(coordinator_chain),
            SEARCH_TOP_K,
        ))
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
        let (transcript_id, message_id, evidence_label) = match &target {
            ConversationReadTarget::StableCurrent { transcript_id } => {
                (transcript_id, None, "current evidence")
            }
            ConversationReadTarget::Exact {
                transcript_id,
                message_id,
            } => (transcript_id, message_id.as_deref(), "exact evidence"),
        };
        let conv = self
            .db
            .get_conversation(transcript_id.as_str())
            .await
            .map_err(|e| format!("transcript not found: {e}"))?;
        if let Some(message_id) = message_id {
            read_conversation_around_message(&self.db, &conv, message_id)
                .await
                .map_err(|e| format!("read failed: {e}"))
        } else {
            read_conversation_page(&self.db, &conv, cursor, evidence_label)
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

    pub(crate) async fn conversation_display_identity(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        let conversation = self
            .db
            .get_conversation(conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let aggregate = self
            .db
            .get_ordinary_product_conversation(&conversation.product_conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let root = aggregate.root.conversation;
        let display_name = root
            .chain_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| root.title.clone().filter(|title| !title.trim().is_empty()))
            .or_else(|| root.slug.clone().filter(|slug| !slug.trim().is_empty()))
            .unwrap_or_else(|| "Untitled conversation".to_string());
        Ok(serde_json::json!({
            "display_name": display_name,
            "transcript_slug": conversation.slug,
        }))
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
    match parse_canonical_conversation_reference(raw, true).map_err(|error| error.to_string())? {
        CanonicalConversationReference::StableProductConversation(id) => {
            let snapshot = service
                .db
                .read_ordinary_product_conversation_snapshot(id.as_str(), None, None, 1)
                .await
                .map_err(|error| {
                    if matches!(error, crate::db::DbError::ConversationNotFound(_)) {
                        "ProductConversation reference not found".to_string()
                    } else {
                        format!("ProductConversation read failed: {error}")
                    }
                })?;
            if snapshot.aggregate.product_conversation.id() != &id {
                return Err("ProductConversation reference not found".to_string());
            }
            Ok(ConversationReadTarget::StableCurrent {
                transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
                    snapshot.aggregate.latest_transcript_row_id,
                )
                .map_err(|error| error.to_string())?,
            })
        }
        CanonicalConversationReference::ExactTranscript {
            transcript_id,
            message_id,
        } => Ok(ConversationReadTarget::Exact {
            transcript_id,
            message_id,
        }),
    }
}

async fn ordinary_product_citation(
    db: &crate::db::Database,
    conv: &Conversation,
) -> Result<Option<(String, String)>, DbError> {
    Ok(db
        .ordinary_product_conversation_citation(&conv.product_conversation_id)
        .await?
        .map(|citation| {
            (
                citation.product_conversation_id.to_string(),
                citation.root_title,
            )
        }))
}

pub(crate) async fn format_global_search_hits(
    service: &GlobalReadService,
    hits: &[crate::db::RetrievedChunk],
) -> Result<String, DbError> {
    let mut out = String::new();
    let mut citations = std::collections::HashMap::<
        phoenix_core::domain::product_conversation::ProductConversationId,
        Option<(String, String)>,
    >::new();
    let mut sender_kinds = std::collections::HashMap::new();
    for hit in hits {
        let (title, stable_reference, href) =
            match service.db.get_conversation(&hit.conversation_id).await {
                Ok(conv) => {
                    let citation = if let Some(citation) =
                        citations.get(&conv.product_conversation_id)
                    {
                        citation.clone()
                    } else {
                        let citation = ordinary_product_citation(&service.db, &conv).await?;
                        citations.insert(conv.product_conversation_id.clone(), citation.clone());
                        citation
                    };
                    let title = citation.as_ref().map_or_else(
                        || {
                            conv.title
                                .clone()
                                .or(conv.slug.clone())
                                .unwrap_or_else(|| conv.id.clone())
                        },
                        |(_, title)| title.clone(),
                    );
                    let stable_reference = citation.map(|(id, _)| format!("@conv:{id}"));
                    let href = Some(conversation_message_href(
                        &conv,
                        Some((&hit.message_id, hit.message_type)),
                    ));
                    (title, stable_reference, href)
                }
                Err(DbError::ConversationNotFound(_)) => (hit.conversation_id.clone(), None, None),
                Err(error) => return Err(error),
            };
        let link = href.unwrap_or_else(|| {
            format!(
                "@transcript:{}#message-{}",
                hit.conversation_id, hit.message_id
            )
        });
        let _ = writeln!(
            out,
            "- [{} · {}{} · {}]({}) {}@transcript:{}#message-{} — {}",
            title,
            attributed_role(hit.message_type, &hit.origin),
            attributed_sender_with_cache(&service.db, &hit.origin, &mut sender_kinds).await?,
            hit.created_at.format("%Y-%m-%d"),
            link,
            stable_reference.map_or_else(String::new, |reference| format!("{reference} · ")),
            hit.conversation_id,
            hit.message_id,
            hit.snippet.trim()
        );
    }
    Ok(out)
}

async fn read_conversation_page(
    db: &crate::db::Database,
    conv: &Conversation,
    cursor: usize,
    evidence_label: &str,
) -> Result<String, DbError> {
    let stable = ordinary_product_citation(db, conv).await?;
    let stable_header = stable.as_ref().map_or_else(String::new, |(id, _)| {
        format!("Conversation @conv:{id}\nconversation link: /product-conversations/{id}\n")
    });
    let title = stable.as_ref().map_or_else(
        || {
            conv.title
                .as_deref()
                .or(conv.slug.as_deref())
                .unwrap_or(&conv.id)
        },
        |(_, title)| title.as_str(),
    );
    let mut header = format!(
        "{stable_header}{evidence_label}: @transcript:{} — {}\nupdated: {}\n---\n",
        conv.id, title, conv.updated_at
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

    let stable = ordinary_product_citation(db, conv).await?;
    let stable_header = stable.as_ref().map_or_else(String::new, |(id, _)| {
        format!("Conversation @conv:{id}\nconversation link: /product-conversations/{id}\n")
    });
    let title = stable.as_ref().map_or_else(
        || {
            conv.title
                .as_deref()
                .or(conv.slug.as_deref())
                .unwrap_or(&conv.id)
        },
        |(_, title)| title.as_str(),
    );
    let mut out = format!(
        "{stable_header}exact evidence: @transcript:{} — {}\nupdated: {}\ntarget_message: {}\nhas_more_before: {}\nhas_more_after: {}\n---\n",
        conv.id,
        title,
        conv.updated_at,
        message_id,
        has_more_before,
        has_more_after,
    );
    let mut sender_kinds = std::collections::HashMap::new();
    for message in messages {
        if !message_is_hidden(&message) {
            out.push_str(
                &render_global_message_line_with_cache(db, conv, &message, &mut sender_kinds)
                    .await?,
            );
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
    let mut sender_kinds = std::collections::HashMap::new();
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
            let line = render_global_message_line_with_cache(db, conv, &message, &mut sender_kinds)
                .await?;
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

pub(crate) fn attributed_role(
    message_type: MessageType,
    origin: &phoenix_core::domain::db_schema::InputOrigin,
) -> &'static str {
    use phoenix_core::domain::db_schema::InputOrigin;
    match message_type {
        MessageType::User => match origin {
            InputOrigin::UserApi => "User API",
            InputOrigin::InternalConversation { .. } => "Conversation",
            InputOrigin::SystemGenerated => "System input",
            InputOrigin::SubscriptionEvent { .. } => "Conversation event",
            InputOrigin::UnknownHistorical => "Unknown input",
        },
        MessageType::Agent => "Agent",
        MessageType::Tool => "Tool",
        MessageType::System => "System",
        MessageType::Error => "Error",
        MessageType::Continuation => "Continuation",
        MessageType::Skill => match origin {
            InputOrigin::UserApi => "Skill · User API",
            InputOrigin::InternalConversation { .. } => "Skill · Conversation",
            InputOrigin::SystemGenerated => "Skill · System input",
            InputOrigin::SubscriptionEvent { .. } => "Skill · Conversation event",
            InputOrigin::UnknownHistorical => "Skill · Unknown input",
        },
    }
}

pub(crate) fn attributed_sender(origin: &phoenix_core::domain::db_schema::InputOrigin) -> String {
    use phoenix_core::domain::db_schema::InputOrigin;
    match origin {
        InputOrigin::InternalConversation {
            product_conversation_id,
            transcript_id,
            ..
        } => format!(
            " from recorded product conversation {product_conversation_id} via @transcript:{transcript_id}"
        ),
        InputOrigin::UnknownHistorical
        | InputOrigin::UserApi
        | InputOrigin::SystemGenerated
        | InputOrigin::SubscriptionEvent { .. } => String::new(),
    }
}

async fn attributed_sender_with_stable(
    db: &crate::db::Database,
    origin: &phoenix_core::domain::db_schema::InputOrigin,
) -> Result<String, DbError> {
    attributed_sender_with_cache(db, origin, &mut std::collections::HashMap::new()).await
}

async fn attributed_sender_with_cache(
    db: &crate::db::Database,
    origin: &phoenix_core::domain::db_schema::InputOrigin,
    sender_kinds: &mut std::collections::HashMap<
        phoenix_core::domain::product_conversation::ProductConversationId,
        Option<phoenix_core::domain::product_conversation::ProductConversationKind>,
    >,
) -> Result<String, DbError> {
    use phoenix_core::domain::db_schema::InputOrigin;
    match origin {
        InputOrigin::InternalConversation {
            product_conversation_id,
            transcript_id,
            ..
        } => {
            let kind = if let Some(kind) = sender_kinds.get(product_conversation_id) {
                *kind
            } else {
                let kind = db
                    .product_conversation_kind(product_conversation_id)
                    .await?;
                sender_kinds.insert(product_conversation_id.clone(), kind);
                kind
            };
            Ok(match kind {
                Some(
                    phoenix_core::domain::product_conversation::ProductConversationKind::Coordinator,
                ) => format!(" from Global via @transcript:{transcript_id}"),
                Some(
                    phoenix_core::domain::product_conversation::ProductConversationKind::Ordinary,
                )
                | None => format!(
                    " from recorded product conversation {product_conversation_id} via @transcript:{transcript_id}"
                ),
            })
        }
        InputOrigin::UnknownHistorical
        | InputOrigin::UserApi
        | InputOrigin::SystemGenerated
        | InputOrigin::SubscriptionEvent { .. } => Ok(String::new()),
    }
}

#[cfg(test)]
async fn render_global_message_line(
    db: &crate::db::Database,
    conv: &Conversation,
    message: &crate::db::Message,
) -> Result<String, DbError> {
    render_global_message_line_with_cache(db, conv, message, &mut std::collections::HashMap::new())
        .await
}

async fn render_global_message_line_with_cache(
    db: &crate::db::Database,
    conv: &Conversation,
    message: &crate::db::Message,
    sender_kinds: &mut std::collections::HashMap<
        phoenix_core::domain::product_conversation::ProductConversationId,
        Option<phoenix_core::domain::product_conversation::ProductConversationKind>,
    >,
) -> Result<String, DbError> {
    let role = attributed_role(message.message_type, &message.origin);
    let href = conversation_message_href(conv, Some((&message.message_id, message.message_type)));
    let sender = attributed_sender_with_cache(db, &message.origin, sender_kinds).await?;
    Ok(format!(
        "[{}{} · {} · {}]({}) @transcript:{}#message-{}\n{}\n\n",
        role,
        sender,
        message.created_at.format("%Y-%m-%d %H:%M"),
        message.message_id,
        href,
        conv.id,
        message.message_id,
        render_full_message_text(message).trim()
    ))
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

fn resolved_work_scope_from_columns(
    work_scope_id: phoenix_core::work_scope::WorkScopeId,
    raw_id: Option<String>,
    raw_lifecycle: Option<&str>,
    raw_environment: Option<&str>,
    cwd: Option<String>,
    worktree_path: Option<String>,
) -> ResolvedWorkScope {
    let Some(raw_id) = raw_id else {
        return ResolvedWorkScope::Unavailable {
            work_scope_id,
            reason: WorkScopeUnavailableReason::RecordNotFound,
        };
    };
    let parsed_id = phoenix_core::work_scope::WorkScopeId::parse(raw_id);
    let lifecycle = match raw_lifecycle {
        Some("active") => Some(phoenix_core::work_scope::WorkScopeLifecycle::Active),
        Some("retired") => Some(phoenix_core::work_scope::WorkScopeLifecycle::Retired),
        _ => None,
    };
    let environment_kind = match raw_environment {
        Some("allocated_worktree") => Some(ResolvedEnvironmentKind::AllocatedWorktree),
        Some("unowned_cwd") => Some(ResolvedEnvironmentKind::UnownedCwd),
        Some("none") => Some(ResolvedEnvironmentKind::None),
        _ => None,
    };
    let (Ok(parsed_id), Some(lifecycle), Some(environment_kind)) =
        (parsed_id, lifecycle, environment_kind)
    else {
        return ResolvedWorkScope::Unavailable {
            work_scope_id,
            reason: WorkScopeUnavailableReason::InvalidRecord,
        };
    };
    let effective_path = worktree_path.clone().or_else(|| cwd.clone());
    ResolvedWorkScope::Available {
        work_scope_id: parsed_id,
        lifecycle,
        environment_kind,
        cwd,
        worktree_path,
        effective_path,
        path_semantics: ServerPathSemantics::ServerFilesystem,
    }
}

struct ResolvedCurrentMember {
    id: String,
    state: crate::db::ConvState,
    updated_at: chrono::DateTime<chrono::Utc>,
    work_scope: ResolvedWorkScope,
}

async fn resolve_current_member_and_work_scope(
    service: &GlobalReadService,
    product_conversation_id: &str,
) -> Result<ResolvedCurrentMember, AppError> {
    let row = sqlx::query(
        "WITH RECURSIVE transcript(id, work_scope_id, ordinal) AS (
             SELECT root.id, root.work_scope_id, 0
             FROM conversations root
             WHERE root.product_conversation_id = ?1
               AND root.runtime_role = 'user'
               AND root.parent_conversation_id IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM conversations predecessor
                   WHERE predecessor.product_conversation_id = root.product_conversation_id
                     AND predecessor.continued_in_conv_id = root.id
               )
             UNION ALL
             SELECT successor.id, successor.work_scope_id, transcript.ordinal + 1
             FROM transcript
             JOIN conversations predecessor ON predecessor.id = transcript.id
             JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
             WHERE successor.product_conversation_id = ?1
               AND successor.runtime_role = 'user'
               AND successor.parent_conversation_id IS NULL
         )
         SELECT transcript.id AS conversation_id, conversation.state,
                conversation.updated_at, transcript.work_scope_id,
                scope.id AS scope_id, scope.lifecycle, scope.environment_kind,
                scope.cwd, scope.worktree_path
         FROM transcript
         JOIN conversations conversation ON conversation.id = transcript.id
         LEFT JOIN work_scopes scope ON scope.id = transcript.work_scope_id
         ORDER BY transcript.ordinal DESC
         LIMIT 1",
    )
    .bind(product_conversation_id)
    .fetch_optional(service.db.pool())
    .await
    .map_err(|error| map_db_not_found(DbError::from(error)))?
    .ok_or_else(|| AppError::NotFound(product_conversation_id.to_string()))?;
    let conversation_id = row
        .try_get("conversation_id")
        .map_err(|error| map_db_not_found(DbError::from(error)))?;
    let work_scope_id: Option<String> = row
        .try_get("work_scope_id")
        .map_err(|error| map_db_not_found(DbError::from(error)))?;
    let work_scope = match work_scope_id {
        None => ResolvedWorkScope::Missing,
        Some(id) => {
            let id = phoenix_core::work_scope::WorkScopeId::parse(id).map_err(|error| {
                AppError::Internal(format!("invalid attached WorkScope ID: {error}"))
            })?;
            let lifecycle: Option<String> = row
                .try_get("lifecycle")
                .map_err(|error| map_db_not_found(DbError::from(error)))?;
            let environment: Option<String> = row
                .try_get("environment_kind")
                .map_err(|error| map_db_not_found(DbError::from(error)))?;
            resolved_work_scope_from_columns(
                id,
                row.try_get("scope_id")
                    .map_err(|error| map_db_not_found(DbError::from(error)))?,
                lifecycle.as_deref(),
                environment.as_deref(),
                row.try_get("cwd")
                    .map_err(|error| map_db_not_found(DbError::from(error)))?,
                row.try_get("worktree_path")
                    .map_err(|error| map_db_not_found(DbError::from(error)))?,
            )
        }
    };
    let raw_state: String = row
        .try_get("state")
        .map_err(|error| map_db_not_found(DbError::from(error)))?;
    let state = serde_json::from_str(&raw_state)
        .map_err(|error| AppError::Internal(format!("invalid conversation state: {error}")))?;
    let raw_updated_at: String = row
        .try_get("updated_at")
        .map_err(|error| map_db_not_found(DbError::from(error)))?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(&raw_updated_at)
        .map_err(|error| AppError::Internal(format!("invalid conversation timestamp: {error}")))?
        .with_timezone(&chrono::Utc);
    Ok(ResolvedCurrentMember {
        id: conversation_id,
        state,
        updated_at,
        work_scope,
    })
}

async fn resolve_work_scope(
    service: &GlobalReadService,
    conversation: &Conversation,
) -> ResolvedWorkScope {
    let Some(work_scope_id) = conversation.attached_work_scope_id.clone() else {
        return ResolvedWorkScope::Missing;
    };
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>)>(
        "SELECT id, lifecycle, environment_kind, cwd, worktree_path
         FROM work_scopes
         WHERE id = ?1",
    )
    .bind(work_scope_id.as_str())
    .fetch_optional(service.db.pool())
    .await;
    let (raw_id, raw_lifecycle, raw_environment, cwd, worktree_path) = match row {
        Ok(Some(row)) => row,
        Ok(None) => {
            return ResolvedWorkScope::Unavailable {
                work_scope_id,
                reason: WorkScopeUnavailableReason::RecordNotFound,
            };
        }
        Err(error) => {
            tracing::warn!(
                work_scope_id = %work_scope_id,
                error = %error,
                "resolve_reference could not read attached WorkScope"
            );
            return ResolvedWorkScope::Unavailable {
                work_scope_id,
                reason: WorkScopeUnavailableReason::ReadFailed,
            };
        }
    };
    resolved_work_scope_from_columns(
        work_scope_id,
        Some(raw_id),
        Some(raw_lifecycle.as_str()),
        Some(raw_environment.as_str()),
        cwd,
        worktree_path,
    )
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
        return Ok(resolve_conversation(service, conv, true).await);
    }
    if let Some(rest) = reference
        .strip_prefix("/chains/")
        .or_else(|| reference.strip_prefix("@chain:"))
    {
        let (id, _) = split_fragment(rest);
        return resolve_chain(service, id).await;
    }
    if let Some(id) = reference
        .strip_prefix("@conv:")
        .or_else(|| reference.strip_prefix("/product-conversations/"))
    {
        if id.contains('#') {
            return Err(AppError::BadRequest(
                "ProductConversation reference must be @conv:<product_conversation_id>".to_string(),
            ));
        }
        let typed_id = phoenix_core::domain::product_conversation::ProductConversationId::parse(id)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let snapshot = service
            .db
            .read_ordinary_product_conversation_snapshot(id, None, None, 1)
            .await
            .map_err(map_db_not_found)?;
        if snapshot.aggregate.product_conversation.id().as_str() != id {
            return Err(AppError::NotFound(id.to_string()));
        }
        let aggregate = snapshot.aggregate;
        #[cfg(test)]
        if let Some(hook) = &service.stable_resolution_test_hook {
            hook.snapshot_read.add_permits(1);
            hook.release
                .acquire()
                .await
                .expect("test hook remains open")
                .forget();
        }
        let current = resolve_current_member_and_work_scope(service, id).await?;
        return Ok(ResolveGlobalReferenceResponse {
            kind: "product_conversation".to_string(),
            id: typed_id.to_string(),
            href: Some(format!("/product-conversations/{typed_id}")),
            title: aggregate
                .root
                .conversation
                .chain_name
                .clone()
                .or(aggregate.root.conversation.title.clone())
                .or(aggregate.root.conversation.slug.clone()),
            work_scope: current.work_scope,
            summary: format!(
                "stable ProductConversation @conv:{typed_id}; root transcript @transcript:{}; current transcript @transcript:{}; state {}; updated {}",
                aggregate.root.conversation.id,
                current.id,
                current.state.variant_name(),
                current.updated_at.to_rfc3339()
            ),
        });
    }
    if let Some(rest) = reference.strip_prefix("@transcript:") {
        let (id, fragment) = split_fragment(rest);
        let message_id = match fragment {
            Some(fragment) => Some(message_id_fragment(fragment).ok_or_else(|| {
                AppError::BadRequest(
                    "transcript fragment must be #message-<message_id>".to_string(),
                )
            })?),
            None => None,
        };
        let conv = service
            .db
            .get_conversation(id)
            .await
            .map_err(map_db_not_found)?;
        if let Some(message_id) = message_id {
            return resolve_message(service, conv, message_id, false).await;
        }
        return Ok(resolve_conversation(service, conv, false).await);
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
    let target = parse_canonical_conversation_reference(raw, false)?;
    if let CanonicalConversationReference::StableProductConversation(product_conversation_id) =
        target
    {
        service
            .db
            .get_ordinary_product_conversation(&product_conversation_id)
            .await
            .map_err(|error| {
                if matches!(error, crate::db::DbError::ConversationNotFound(_)) {
                    GlobalMessageTargetError::ConversationNotFound(
                        product_conversation_id.to_string(),
                    )
                } else {
                    GlobalMessageTargetError::ResolutionFailed(error.to_string())
                }
            })?;
        return Ok(GlobalMessageTarget::StableProductConversation {
            product_conversation_id,
        });
    }
    let CanonicalConversationReference::ExactTranscript { transcript_id, .. } = target else {
        unreachable!("stable target returned above")
    };
    let conversation = service
        .db
        .get_conversation(transcript_id.as_str())
        .await
        .map_err(|error| {
            if matches!(error, crate::db::DbError::ConversationNotFound(_)) {
                GlobalMessageTargetError::ConversationNotFound(transcript_id.to_string())
            } else {
                GlobalMessageTargetError::ResolutionFailed(error.to_string())
            }
        })?;
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
        transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
            conversation.id,
        )
        .map_err(|error| GlobalMessageTargetError::ResolutionFailed(error.to_string()))?,
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
        .filter(|id| !id.is_empty() && !id.contains('#'))
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

async fn resolve_conversation(
    service: &GlobalReadService,
    conv: Conversation,
    global_href: bool,
) -> ResolveGlobalReferenceResponse {
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
        work_scope: resolve_work_scope(service, &conv).await,
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
    let sender = attributed_sender_with_stable(&service.db, &message.origin)
        .await
        .map_err(|error| AppError::Internal(format!("sender attribution failed: {error}")))?;
    Ok(ResolveGlobalReferenceResponse {
        kind: "message".to_string(),
        id: message.message_id.clone(),
        href,
        title,
        work_scope: resolve_work_scope(service, &conv).await,
        summary: format!(
            "{}{} message {} in @transcript:{} at {}: {}",
            attributed_role(message.message_type, &message.origin),
            sender,
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
    let selected = match members.last() {
        Some(current_id) => service
            .db
            .get_conversation(current_id)
            .await
            .map_err(map_db_not_found)?,
        None => root.clone(),
    };
    let work_scope = resolve_work_scope(service, &selected).await;
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
        work_scope,
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
        work_scope: resolve_work_scope(service, current).await,
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
        format_global_search_hits, message_id_fragment, render_full_message_text,
        render_global_message_line, resolve_conversation_read_target, resolve_reference_impl,
        resolve_work_scope, split_fragment, ConversationReadTarget,
        CoordinatorWorkScopeTargetError, GlobalMessageTarget, GlobalMessageTargetError,
        GlobalReadService, ResolvedEnvironmentKind, ResolvedWorkScope, ServerPathSemantics,
        WorkScopeUnavailableReason,
    };
    use std::sync::Arc;

    use crate::db::MessageRetriever;

    #[tokio::test]
    async fn compatibility_resolver_rejects_blank_product_and_bare_references_without_panicking() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));

        for reference in ["@conv:   ", "/product-conversations/   ", "bare-id"] {
            let error = resolve_reference_impl(&service, reference)
                .await
                .unwrap_err();
            assert!(
                matches!(error, crate::api::handlers::AppError::BadRequest(_)),
                "{reference}: {error:?}"
            );
        }
    }

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

    #[tokio::test]
    async fn global_message_line_attributes_server_recorded_sender_not_user_role() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("origin-reader", "origin-reader", "/tmp", true, None, None)
            .await
            .unwrap();
        let sender = db
            .create_conversation("origin-sender", "origin-sender", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut message = db
            .add_message(
                "origin-reader-message",
                &conv.id,
                &MessageContent::user("message body"),
                None,
                None,
            )
            .await
            .unwrap();
        let source = InputOrigin::InternalConversation {
            product_conversation_id: sender.product_conversation_id.clone(),
            transcript_id: sender.id.clone(),
            source_call: None,
        };
        message.origin = source;
        let rendered = render_global_message_line(&db, &conv, &message)
            .await
            .unwrap();
        assert!(rendered.contains(&format!(
            "Conversation from recorded product conversation {} via @transcript:{}",
            sender.product_conversation_id, sender.id
        )));
        assert!(!rendered.contains("User API"));

        message.origin = InputOrigin::UnknownHistorical;
        let rendered = render_global_message_line(&db, &conv, &message)
            .await
            .unwrap();
        assert!(rendered.contains("Unknown input"));
        assert!(!rendered.contains("User API"));
    }

    #[tokio::test]
    async fn sender_kind_cache_reuses_classification_within_one_render() {
        use phoenix_core::domain::db_schema::InputOrigin;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let origin = InputOrigin::InternalConversation {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: None,
        };
        let mut cache = std::collections::HashMap::new();

        let first = super::attributed_sender_with_cache(&db, &origin, &mut cache)
            .await
            .unwrap();
        db.delete_conversation(&coordinator.id).await.unwrap();
        let second = super::attributed_sender_with_cache(&db, &origin, &mut cache)
            .await
            .unwrap();

        assert_eq!(first, second);
        assert!(second.contains("from Global"));
    }

    #[tokio::test]
    async fn sender_attribution_propagates_classification_failures() {
        use phoenix_core::domain::db_schema::InputOrigin;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let sender = db
            .create_conversation(
                "sender-error-source",
                "sender-error-source",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let origin = InputOrigin::InternalConversation {
            product_conversation_id: sender.product_conversation_id,
            transcript_id: sender.id,
            source_call: None,
        };
        db.pool().close().await;

        let error = super::attributed_sender_with_stable(&db, &origin)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("closed"), "{error}");
    }

    #[tokio::test]
    async fn conversation_read_propagates_citation_database_failures() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("citation-error", "citation-error", "/tmp", true, None, None)
            .await
            .unwrap();
        db.pool().close().await;

        let error = super::read_conversation_page(&db, &conv, 0, "evidence")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("closed"), "{error}");
    }

    #[tokio::test]
    async fn global_message_line_retains_recorded_stable_sender_after_hard_delete() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let reader = db
            .create_conversation("deleted-reader", "deleted-reader", "/tmp", true, None, None)
            .await
            .unwrap();
        let sender = db
            .create_conversation("deleted-sender", "deleted-sender", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut message = db
            .add_message(
                "deleted-sender-message",
                &reader.id,
                &MessageContent::user("message body"),
                None,
                None,
            )
            .await
            .unwrap();
        message.origin = InputOrigin::InternalConversation {
            product_conversation_id: sender.product_conversation_id.clone(),
            transcript_id: sender.id.clone(),
            source_call: None,
        };

        let before_delete = render_global_message_line(&db, &reader, &message)
            .await
            .unwrap();
        db.delete_conversation(&sender.id).await.unwrap();

        let rendered = render_global_message_line(&db, &reader, &message)
            .await
            .unwrap();
        assert_eq!(rendered, before_delete);
        assert!(rendered.contains(&format!(
            "Conversation from recorded product conversation {} via @transcript:{}",
            sender.product_conversation_id, sender.id
        )));
        assert!(!rendered.contains("from @conv:"));
        assert!(!rendered.contains("from Global"));
    }

    #[tokio::test]
    async fn global_message_line_attributes_live_coordinator_sender_as_global() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let reader = db
            .create_conversation("global-reader", "global-reader", "/tmp", true, None, None)
            .await
            .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let mut message = db
            .add_message(
                "global-sender-message",
                &reader.id,
                &MessageContent::user("message body"),
                None,
                None,
            )
            .await
            .unwrap();
        message.origin = InputOrigin::InternalConversation {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: None,
        };

        let rendered = render_global_message_line(&db, &reader, &message)
            .await
            .unwrap();
        assert!(rendered.contains(&format!(
            "Conversation from Global via @transcript:{}",
            coordinator.id
        )));
        assert!(!rendered.contains(&format!("@conv:{}", coordinator.product_conversation_id)));
    }

    #[tokio::test]
    async fn search_hit_uses_same_recorded_attribution_as_full_read() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent, MessageType};
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("search-origin", "search-origin", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut message = db
            .add_message(
                "search-origin-message",
                &conv.id,
                &MessageContent::user("search origin body"),
                None,
                None,
            )
            .await
            .unwrap();
        let retriever = Arc::new(db.fts_retriever());
        let mut hit = retriever
            .retrieve(crate::db::RetrievalRequest::natural_language(
                "search origin",
                crate::db::RetrievalScope::Global,
                10,
            ))
            .await
            .unwrap()
            .remove(0);
        let service = GlobalReadService::new(db, retriever);
        let cases = [
            (InputOrigin::UnknownHistorical, "Unknown input"),
            (InputOrigin::UserApi, "User API"),
            (InputOrigin::SystemGenerated, "System input"),
            (
                InputOrigin::SubscriptionEvent {
                    event_id: "event-1".into(),
                },
                "Conversation event",
            ),
            (
                InputOrigin::InternalConversation {
                    product_conversation_id: conv.product_conversation_id.clone(),
                    transcript_id: conv.id.clone(),
                    source_call: None,
                },
                "Conversation from recorded product conversation ",
            ),
        ];
        for (origin, expected) in cases {
            message.origin = origin.clone();
            hit.origin = origin;
            let search = format_global_search_hits(&service, &[hit.clone()])
                .await
                .unwrap();
            let full = render_global_message_line(&service.db, &conv, &message)
                .await
                .unwrap();
            assert!(search.contains(&format!("@conv:{}", conv.product_conversation_id)));
            assert!(search.contains(&format!(
                "@transcript:{}#message-{}",
                hit.conversation_id, hit.message_id
            )));
            assert!(search.contains(expected), "{search}");
            assert!(full.contains(expected), "{full}");
            if expected != "User API" {
                assert!(!search.contains("User API"), "{search}");
            }
            message.message_type = MessageType::Skill;
            hit.message_type = MessageType::Skill;
            let search = format_global_search_hits(&service, &[hit.clone()])
                .await
                .unwrap();
            let full = render_global_message_line(&service.db, &conv, &message)
                .await
                .unwrap();
            let skill_label = format!("Skill · {expected}");
            assert!(search.contains(&skill_label), "{search}");
            assert!(full.contains(&skill_label), "{full}");
            message.message_type = MessageType::User;
            hit.message_type = MessageType::User;
        }
    }

    #[tokio::test]
    async fn search_formatting_propagates_transcript_lookup_failures() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let service = super::GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));
        let hit = crate::db::RetrievedChunk {
            message_id: "lookup-error-message".to_string(),
            conversation_id: "lookup-error-conversation".to_string(),
            chunk: crate::db::ChunkRef {
                ordinal: 0,
                char_range: None,
            },
            message_type: phoenix_core::domain::db_schema::MessageType::User,
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
            created_at: chrono::Utc::now(),
            snippet: "message body".to_string(),
            score: 0.0,
            transcript_generation: 0,
            message_count: 1,
        };
        db.pool().close().await;

        let error = super::format_global_search_hits(&service, &[hit])
            .await
            .unwrap_err();

        assert!(error.to_string().contains("closed"), "{error}");
    }

    #[tokio::test]
    async fn search_citation_uses_aggregate_root_title_not_successor_local_title() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET title = 'Aggregate Title', state = '{\"type\":\"context_exhausted\",\"summary\":\"continue\"}', state_kind = 'context_exhausted' WHERE id = 'root'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET chain_name = 'Effective Rename' WHERE id = 'root'")
            .execute(db.pool())
            .await
            .unwrap();
        let current = match db.continue_conversation("root").await.unwrap() {
            crate::db::ContinueOutcome::Created(current) => current,
            crate::db::ContinueOutcome::AlreadyContinued(current) => {
                panic!("unexpected existing continuation: {current:?}")
            }
            crate::db::ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("parent unexpectedly remained in state {state_variant}")
            }
        };
        sqlx::query("UPDATE conversations SET title = 'Successor Local Title' WHERE id = ?1")
            .bind(&current.id)
            .execute(db.pool())
            .await
            .unwrap();
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));
        let hit = crate::db::RetrievedChunk {
            conversation_id: current.id,
            message_id: "message".to_string(),
            chunk: crate::db::ChunkRef {
                ordinal: 0,
                char_range: None,
            },
            message_type: crate::db::MessageType::User,
            created_at: chrono::Utc::now(),
            snippet: "evidence".to_string(),
            origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
            score: 0.0,
            transcript_generation: 0,
            message_count: 1,
        };

        let output = format_global_search_hits(&service, &[hit]).await.unwrap();

        assert!(output.contains("Effective Rename"), "{output}");
        assert!(!output.contains("Aggregate Title"), "{output}");
        assert!(!output.contains("Successor Local Title"), "{output}");
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
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
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
        assert_eq!(message_id_fragment("message-id#extra"), None);
    }

    #[test]
    fn scoped_attribution_retains_recorded_stable_sender_without_classifying_it() {
        let origin = phoenix_core::domain::db_schema::InputOrigin::InternalConversation {
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::parse(
                    "sender-product",
                )
                .unwrap(),
            transcript_id: "sender-transcript".to_string(),
            source_call: None,
        };

        let rendered = super::attributed_sender(&origin);

        assert_eq!(
            rendered,
            " from recorded product conversation sender-product via @transcript:sender-transcript"
        );
        assert!(!rendered.contains("@conv:"));
        assert!(!rendered.contains("Global"));
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

        assert!(matches!(
            &stable,
            ConversationReadTarget::StableCurrent { transcript_id }
                if transcript_id.as_str() == current.id
        ));
        assert!(matches!(
            &exact,
            ConversationReadTarget::Exact { transcript_id, message_id: None }
                if transcript_id.as_str() == "root"
        ));
        let stable_output = service
            .read_conversation(&format!("@conv:{product_id}"), 0)
            .await
            .unwrap();
        let exact_output = service
            .read_conversation("@transcript:root", 0)
            .await
            .unwrap();
        assert!(stable_output.contains(&format!("Conversation @conv:{product_id}")));
        assert!(stable_output.contains(&format!("current evidence: @transcript:{}", current.id)));
        assert!(!stable_output.contains("current evidence: @transcript:root"));
        assert!(exact_output.contains("exact evidence: @transcript:root"));
        assert!(!exact_output.contains(&format!("exact evidence: @transcript:{}", current.id)));
        assert!(resolve_conversation_read_target(&service, "@conv:root")
            .await
            .unwrap_err()
            .contains("not found"));
        assert_eq!(
            resolve_conversation_read_target(&service, "root")
                .await
                .unwrap_err(),
            GlobalMessageTargetError::UnsupportedSyntax.to_string()
        );
    }

    #[tokio::test]
    async fn reference_resolution_reports_exact_selected_scope_and_server_paths() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("root-scope", "root-scope", "/tmp/root", true, None, None)
            .await
            .unwrap();
        let root = db.get_conversation("root-scope").await.unwrap();
        let root_scope_id = root.attached_work_scope_id.clone().unwrap();
        let product_id = root.product_conversation_id.clone();
        sqlx::query(
            "UPDATE work_scopes
             SET lifecycle = 'retired', retired_at = '2026-01-01T00:00:00Z',
                 environment_kind = 'allocated_worktree', cwd = '/server/root',
                 worktree_path = '/server/root/worktree', branch_name = 'feature/root',
                 base_branch = 'main'
             WHERE id = ?1",
        )
        .bind(root_scope_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET state = '{\"type\":\"context_exhausted\",\"summary\":\"continue\"}', state_kind = 'context_exhausted' WHERE id = 'root-scope'")
            .execute(db.pool())
            .await
            .unwrap();
        let current = match db.continue_conversation("root-scope").await.unwrap() {
            crate::db::ContinueOutcome::Created(current) => current,
            crate::db::ContinueOutcome::AlreadyContinued(current) => {
                panic!("unexpected existing continuation: {current:?}")
            }
            crate::db::ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("unexpected parent state: {state_variant}")
            }
        };
        let current_scope_id =
            phoenix_core::work_scope::WorkScopeId::parse("current-scope").unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, lifecycle, environment_kind, cwd,
                 created_at, updated_at
             ) VALUES (?1, 'work', 'active', 'unowned_cwd', '/server/current', ?2, ?2)",
        )
        .bind(current_scope_id.as_str())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = ?2")
            .bind(current_scope_id.as_str())
            .bind(&current.id)
            .execute(db.pool())
            .await
            .unwrap();
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));

        let stable = service
            .resolve_reference(&format!("@conv:{product_id}"))
            .await
            .unwrap();
        assert_eq!(
            stable.work_scope,
            ResolvedWorkScope::Available {
                work_scope_id: current_scope_id,
                lifecycle: phoenix_core::work_scope::WorkScopeLifecycle::Active,
                environment_kind: ResolvedEnvironmentKind::UnownedCwd,
                cwd: Some("/server/current".to_string()),
                worktree_path: None,
                effective_path: Some("/server/current".to_string()),
                path_semantics: ServerPathSemantics::ServerFilesystem,
            }
        );
        let stable_json = serde_json::to_value(&stable).unwrap();
        assert_eq!(stable_json["work_scope"]["status"], "available");
        assert_eq!(
            stable_json["work_scope"]["path_semantics"],
            "server_filesystem"
        );

        let exact = service
            .resolve_reference("@transcript:root-scope")
            .await
            .unwrap();
        assert_eq!(
            exact.work_scope,
            ResolvedWorkScope::Available {
                work_scope_id: root_scope_id,
                lifecycle: phoenix_core::work_scope::WorkScopeLifecycle::Retired,
                environment_kind: ResolvedEnvironmentKind::AllocatedWorktree,
                cwd: Some("/server/root".to_string()),
                worktree_path: Some("/server/root/worktree".to_string()),
                effective_path: Some("/server/root/worktree".to_string()),
                path_semantics: ServerPathSemantics::ServerFilesystem,
            }
        );
    }

    #[tokio::test]
    async fn stable_resolution_keeps_current_member_and_scope_from_one_point_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stable-resolution.sqlite");
        let db = crate::db::Database::open(path.to_str().unwrap())
            .await
            .unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation("race-root", "race-root", "/tmp/root", true, None, None)
            .await
            .unwrap();
        let root = db.get_conversation("race-root").await.unwrap();
        let product_id = root.product_conversation_id.clone();
        sqlx::query("UPDATE conversations SET state = '{\"type\":\"context_exhausted\",\"summary\":\"continue\"}', state_kind = 'context_exhausted' WHERE id = 'race-root'")
            .execute(db.pool())
            .await
            .unwrap();
        let mut service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));
        let hook = service.install_stable_resolution_test_hook();
        let reference = format!("@conv:{product_id}");
        let resolving = tokio::spawn(async move { service.resolve_reference(&reference).await });
        hook.snapshot_read.acquire().await.unwrap().forget();

        let successor = match db.continue_conversation("race-root").await.unwrap() {
            crate::db::ContinueOutcome::Created(successor) => successor,
            crate::db::ContinueOutcome::AlreadyContinued(successor) => {
                panic!("unexpected existing continuation: {successor:?}")
            }
            crate::db::ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("unexpected parent state: {state_variant}")
            }
        };
        hook.release.add_permits(1);
        let response = resolving.await.unwrap().unwrap();

        assert!(response
            .summary
            .contains(&format!("@transcript:{}", successor.id)));
        assert_eq!(
            response.work_scope,
            ResolvedWorkScope::Available {
                work_scope_id: successor.attached_work_scope_id.unwrap(),
                lifecycle: phoenix_core::work_scope::WorkScopeLifecycle::Active,
                environment_kind: ResolvedEnvironmentKind::UnownedCwd,
                cwd: Some("/tmp/root".to_string()),
                worktree_path: None,
                effective_path: Some("/tmp/root".to_string()),
                path_semantics: ServerPathSemantics::ServerFilesystem,
            }
        );

        let historical = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()))
            .resolve_reference("@transcript:race-root")
            .await
            .unwrap();
        assert_eq!(historical.id, "race-root");
        assert_eq!(
            historical.work_scope,
            ResolvedWorkScope::Available {
                work_scope_id: root.attached_work_scope_id.unwrap(),
                lifecycle: phoenix_core::work_scope::WorkScopeLifecycle::Active,
                environment_kind: ResolvedEnvironmentKind::UnownedCwd,
                cwd: Some("/tmp/root".to_string()),
                worktree_path: None,
                effective_path: Some("/tmp/root".to_string()),
                path_semantics: ServerPathSemantics::ServerFilesystem,
            }
        );
    }

    #[tokio::test]
    async fn work_scope_resolution_distinguishes_missing_unavailable_and_no_environment() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.create_conversation("scope-status", "scope-status", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut conversation = db.get_conversation("scope-status").await.unwrap();
        let scope_id = conversation.attached_work_scope_id.clone().unwrap();
        let service = GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));

        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'none', cwd = NULL, worktree_path = NULL,
                 branch_name = NULL, base_branch = NULL
             WHERE id = ?1",
        )
        .bind(scope_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            resolve_work_scope(&service, &conversation).await,
            ResolvedWorkScope::Available {
                work_scope_id: scope_id,
                lifecycle: phoenix_core::work_scope::WorkScopeLifecycle::Active,
                environment_kind: ResolvedEnvironmentKind::None,
                cwd: None,
                worktree_path: None,
                effective_path: None,
                path_semantics: ServerPathSemantics::ServerFilesystem,
            }
        );

        conversation.attached_work_scope_id = None;
        assert_eq!(
            resolve_work_scope(&service, &conversation).await,
            ResolvedWorkScope::Missing
        );

        conversation.attached_work_scope_id =
            Some(phoenix_core::work_scope::WorkScopeId::parse("absent-scope").unwrap());
        assert_eq!(
            resolve_work_scope(&service, &conversation).await,
            ResolvedWorkScope::Unavailable {
                work_scope_id: phoenix_core::work_scope::WorkScopeId::parse("absent-scope")
                    .unwrap(),
                reason: WorkScopeUnavailableReason::RecordNotFound,
            }
        );
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
                product_conversation_id: product_id.clone(),
            }
        );
        assert_eq!(
            service
                .resolve_message_target("@transcript:root")
                .await
                .unwrap(),
            GlobalMessageTarget::ExactTranscript {
                transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse("root")
                    .unwrap(),
            }
        );
        for rejected in [
            "root",
            "/c/root",
            "@work:root",
            "@chain:root",
            "@conv:root#message-id",
            "@conv:   ",
            "@transcript:   ",
            "@conv:root extra",
            "@transcript:root extra",
            "@transcript:root#message-id#extra",
            " @conv:root",
            "@conv:root ",
            " @transcript:root",
            "@transcript:root ",
        ] {
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
        assert_eq!(
            service
                .resolve_active_work_scope_bash_target(work_scope_id.as_str())
                .await
                .unwrap_err(),
            CoordinatorWorkScopeTargetError::Authority
        );
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
        assert_eq!(
            service
                .resolve_active_work_scope_bash_target(work_scope_id.as_str())
                .await
                .unwrap_err(),
            CoordinatorWorkScopeTargetError::Authority
        );
    }
}
