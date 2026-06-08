#![allow(clippy::wildcard_enum_match_arm)]
//! Chain Q&A backend (REQ-CHN-001, REQ-CHN-004, REQ-CHN-005, REQ-CHN-006,
//! REQ-CHN-009).
//!
//! Each question runs a **read-only agentic loop** (REQ-CHN-009): a fresh
//! agent is given two scope-bound tools — `search_conversations` (ranked
//! retrieval over the chain via `specs/conversation-retrieval/`) and
//! `read_conversation` (byte-budgeted full-content read of one member) — plus
//! a lightweight chain skeleton for orientation. It iterates (search → read →
//! search) until it can answer, then the final answer streams over the
//! chain-scoped SSE broadcaster. The agent has no state-mutating tools and is
//! bound to the chain's members, so it can dig arbitrarily deep within the
//! chain but cannot reach outside it or change anything. Each question is a
//! fresh run with no memory of prior Q&A (REQ-CHN-006). The Q&A row is
//! persisted through its lifecycle (`in_flight` → `completed` | `failed`).

use crate::chain_runtime::{ChainRuntime, ChainRuntimeRegistry, ChainSseEvent};
use crate::db::{
    ChainQaRow, Conversation, Database, DbError, Message, MessageContent, MessageRetriever,
    MessageType, NewChainQa, RetrievalScope, RetrievedChunk,
};
use crate::llm::{
    ContentBlock, LlmError, LlmMessage, LlmRequest, LlmService, MessageRole, ModelRegistry,
    PromptCacheKey, SystemContent, TokenChunk, ToolDefinition,
};
use chrono::Utc;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Maximum number of tool-using turns the Q&A agent may take before it must
/// answer (REQ-CHN-009). Bounds cost/latency so the loop terminates; the
/// agent typically searches once or twice and reads a member or two.
const MAX_QA_TURNS: usize = 6;

/// How many ranked chunks `search_conversations` returns per call.
const SEARCH_TOP_K: usize = 8;

/// Host-fixed budget (in characters) for one `read_conversation` page
/// (REQ-RET-008): the model cannot enlarge it, so a single read can never
/// overflow the next turn's context. Continuation is by character offset, so
/// the bound holds even within one oversized message.
const READ_PAGE_CHARS: usize = 6000;

/// Maximum tokens cap for an answer turn. Sized to a typical recall answer;
/// the model can stop earlier via `end_turn`.
const ANSWER_MAX_TOKENS: u32 = 2048;

/// Snapshot of chain shape captured at answer time (REQ-CHN-005). Two integers
/// stand in for a full member-graph snapshot; the UI compares them against
/// current chain state to show an age-of-answer freshness tag (whether the
/// chain has grown since this answer was produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainSnapshot {
    pub member_count: i64,
    pub total_messages: i64,
}

/// Compute the snapshot integers from an ordered list of chain members.
///
/// `Conversation::message_count` is a query-time computed field (see
/// `parse_conversation_row` in `src/db.rs`), populated when the row is
/// loaded; we sum those values rather than re-querying.
pub fn compute_chain_snapshot(members: &[Conversation]) -> ChainSnapshot {
    ChainSnapshot {
        member_count: i64::try_from(members.len()).unwrap_or(i64::MAX),
        total_messages: members.iter().map(|c| c.message_count).sum(),
    }
}

/// Errors surfaced by the chain Q&A backend.
#[derive(thiserror::Error, Debug)]
pub enum ChainQaError {
    /// `root_conv_id` is not a chain root (no predecessor allowed; chain
    /// length must be ≥ 2 — single conversations are not chains).
    #[error("conversation {0} is not a chain root or chain has fewer than 2 members")]
    NotAChainRoot(String),
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("LLM error: {0}")]
    Llm(String),
    #[error("no mid-tier LLM model available — registry has no models")]
    NoModelAvailable,
}

impl From<LlmError> for ChainQaError {
    fn from(e: LlmError) -> Self {
        Self::Llm(e.message)
    }
}

/// Identifier returned to the caller of [`ChainQa::submit_question`] — doubles
/// as the SSE-stream demux key on the chain broadcaster.
pub type ChainQaId = String;

/// Chain Q&A entry point.
///
/// `submit_question` returns the `chain_qa_id` synchronously — once the
/// `chain_qa` row is inserted in `in_flight` — and runs the agent loop plus DB
/// finalize in a detached `tokio::spawn`'d task, mirroring how Phoenix's
/// per-conversation runtime returns a handle synchronously and runs the
/// executor in a spawned task.
#[derive(Clone)]
pub struct ChainQa {
    db: Database,
    llm_registry: Arc<ModelRegistry>,
    /// Scope-filtered message retrieval the Q&A agent drives as a tool
    /// (`specs/conversation-retrieval/` REQ-RET-008, chains REQ-CHN-009).
    retriever: Arc<dyn MessageRetriever>,
    runtime_registry: ChainRuntimeRegistry,
}

impl ChainQa {
    pub fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        retriever: Arc<dyn MessageRetriever>,
    ) -> Self {
        Self {
            db,
            llm_registry,
            retriever,
            runtime_registry: ChainRuntimeRegistry::new(),
        }
    }

    /// Construct with an externally-owned chain runtime registry. Production
    /// code shares one registry across the API + handler layer so SSE
    /// subscribers and Q&A submissions go through the same broadcasters.
    #[allow(dead_code)] // Reserved for callers that need a registry shared across multiple ChainQa values.
    pub fn with_registry(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        retriever: Arc<dyn MessageRetriever>,
        runtime_registry: ChainRuntimeRegistry,
    ) -> Self {
        Self {
            db,
            llm_registry,
            retriever,
            runtime_registry,
        }
    }

    /// Read-side: registry handle so HTTP/SSE handlers can subscribe to the
    /// same broadcasters this service publishes onto.
    pub fn runtime_registry(&self) -> &ChainRuntimeRegistry {
        &self.runtime_registry
    }

    /// Submit a question on the chain rooted at `root_id`. Returns the
    /// `chain_qa_id` synchronously — once the `chain_qa` row is inserted in
    /// `in_flight` — and runs the agent loop plus DB finalize in a detached
    /// task. The id doubles as the SSE-stream demux key.
    pub async fn submit_question(
        &self,
        root_id: &str,
        question: &str,
    ) -> Result<ChainQaId, ChainQaError> {
        let prep = self.prepare_invocation(root_id, question).await?;
        let qa_id_for_caller = prep.row_id.clone();
        let qa_id_for_task = prep.row_id.clone();

        // Pin the chain runtime alive for the streaming window: the in-flight
        // guard must be acquired before submit_question returns so a fast
        // subscriber can't trip release_if_idle between insert and the
        // spawned task starting.
        let runtime = self.runtime_registry.get_or_create(root_id).await;
        let in_flight_guard = runtime.begin_qa();

        let this = self.clone();
        let runtime_for_task = Arc::clone(&runtime);
        tokio::spawn(async move {
            let invocation_result = this.run_answer_invocation(&prep, &runtime_for_task).await;
            this.finalize(&qa_id_for_task, invocation_result, &runtime_for_task)
                .await;
            drop(in_flight_guard);
            this.runtime_registry
                .release_if_idle(runtime_for_task.root_conv_id())
                .await;
        });

        Ok(qa_id_for_caller)
    }

    /// Test/foreground-driven variant: runs the agent loop and finalize in the
    /// current task instead of spawning. Used by integration tests that need
    /// deterministic completion before asserting on the persisted row.
    #[cfg(test)]
    pub async fn submit_question_blocking(
        &self,
        root_id: &str,
        question: &str,
    ) -> Result<ChainQaId, ChainQaError> {
        let prep = self.prepare_invocation(root_id, question).await?;
        let qa_id = prep.row_id.clone();

        let runtime = self.runtime_registry.get_or_create(root_id).await;
        let in_flight_guard = runtime.begin_qa();

        let invocation_result = self.run_answer_invocation(&prep, &runtime).await;
        self.finalize(&qa_id, invocation_result, &runtime).await;
        drop(in_flight_guard);
        self.runtime_registry
            .release_if_idle(runtime.root_conv_id())
            .await;

        Ok(qa_id)
    }

    /// Phase 1 of the submission flow.
    ///
    /// Validates the chain, snapshots its shape, builds the orientation
    /// skeleton, and INSERTs the row in `in_flight` — all *before* the agent
    /// loop fires, so the question is durable even if the loop panics
    /// mid-flight (REQ-CHN-005: question text is preserved across failures).
    async fn prepare_invocation(
        &self,
        root_id: &str,
        question: &str,
    ) -> Result<PreparedInvocation, ChainQaError> {
        // Validate: root_id must self-resolve under chain_root_of (i.e. have
        // no predecessor) AND have ≥ 2 forward members (REQ-CHN-002:
        // single-member conversations are not chains).
        let root = self.db.chain_root_of(root_id).await?;
        if root.as_deref() != Some(root_id) {
            return Err(ChainQaError::NotAChainRoot(root_id.to_string()));
        }

        let member_ids = self.db.chain_members_forward(root_id).await?;
        if member_ids.len() < 2 {
            return Err(ChainQaError::NotAChainRoot(root_id.to_string()));
        }

        let mut members: Vec<Conversation> = Vec::with_capacity(member_ids.len());
        for id in &member_ids {
            members.push(self.db.get_conversation(id).await?);
        }
        let snapshot = compute_chain_snapshot(&members);

        let (model_id, service) = self
            .llm_registry
            .get_mid_tier_model()
            .ok_or(ChainQaError::NoModelAvailable)?;

        // The chain root pins the language for all members (continuations
        // inherit it at creation time).
        let language = members.first().map(|c| c.llm_language).unwrap_or_default();
        let skeleton = self.build_chain_skeleton(&members).await?;

        let qa_id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now();
        self.db
            .insert_chain_qa(NewChainQa {
                id: qa_id.clone(),
                root_conv_id: root_id.to_string(),
                question: question.to_string(),
                model: model_id.clone(),
                snapshot_member_count: snapshot.member_count,
                snapshot_total_messages: snapshot.total_messages,
                created_at,
            })
            .await?;

        Ok(PreparedInvocation {
            row_id: qa_id,
            question: question.to_string(),
            skeleton,
            member_ids,
            service,
            model_id,
            language,
        })
    }

    /// Build the orientation skeleton: one line per member with its id, title,
    /// and trailing continuation summary (or a "latest member" marker for the
    /// leaf). Substance comes from the tools, not the skeleton; this just
    /// orients the agent (REQ-CHN-009).
    async fn build_chain_skeleton(&self, members: &[Conversation]) -> Result<String, ChainQaError> {
        let leaf_idx = members.len().saturating_sub(1);
        let mut out = String::new();
        for (i, conv) in members.iter().enumerate() {
            let title = conv
                .title
                .as_deref()
                .or(conv.slug.as_deref())
                .unwrap_or(&conv.id);
            let note = if i == leaf_idx {
                "(latest / current member)".to_string()
            } else {
                let msgs = self.db.get_messages(&conv.id).await?;
                trailing_continuation_summary(&msgs)
                    .unwrap_or_else(|| "(no continuation summary persisted)".to_string())
            };
            let _ = writeln!(out, "- #{} \"{}\": {}", conv.id, title, note.trim());
        }
        Ok(out)
    }

    /// Phase 2 — the read-only agent loop (REQ-CHN-009).
    ///
    /// Seeds the model with the question + skeleton and the two scope-bound
    /// tools, then loops: each turn streams into a per-turn buffer. A turn that
    /// ends in tool calls is **intermediate** — its buffered text is discarded
    /// (streaming discipline: only the final answer reaches the user), its
    /// tool calls are executed, and their results are appended. A turn with no
    /// tool call is the **answer**: its buffered tokens are published. The last
    /// allowed turn offers no tools, forcing an answer (bounded cost).
    async fn run_answer_invocation(
        &self,
        prep: &PreparedInvocation,
        runtime: &Arc<ChainRuntime>,
    ) -> Result<String, RunInvocationError> {
        let mut messages = vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(format!(
                "Chain skeleton (members in order):\n{}\n---\nQuestion: {}",
                prep.skeleton, prep.question
            ))],
        }];

        for turn in 0..MAX_QA_TURNS {
            let force_answer = turn + 1 == MAX_QA_TURNS;
            let request = build_agent_request(&messages, prep.language, force_answer);

            // Per-turn buffer: collect this turn's deltas without publishing.
            // We only know whether the turn is intermediate (tool calls) or
            // the final answer after it completes.
            let (chunk_tx, mut chunk_rx) = broadcast::channel::<TokenChunk>(256);
            let collector = tokio::spawn(async move {
                let mut deltas: Vec<String> = Vec::new();
                loop {
                    match chunk_rx.recv().await {
                        Ok(TokenChunk::Text(d)) => deltas.push(d),
                        Err(broadcast::error::RecvError::Closed) => break,
                        Ok(TokenChunk::RateLimitSnapshot(_))
                        | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    }
                }
                deltas
            });

            let response = prep.service.complete_streaming(&request, &chunk_tx).await;
            drop(chunk_tx);
            let deltas = collector.await.unwrap_or_default();

            let resp = match response {
                Ok(r) => r,
                Err(e) => {
                    // Publish whatever streamed before the error so the user
                    // sees the partial (REQ-CHN-005), then fail.
                    Self::publish_deltas(runtime, &prep.row_id, &deltas);
                    let partial = deltas.concat();
                    return Err(RunInvocationError {
                        error: ChainQaError::from(e),
                        partial_answer: (!partial.is_empty()).then_some(partial),
                    });
                }
            };

            let tool_calls: Vec<(String, String, serde_json::Value)> = resp
                .tool_uses()
                .into_iter()
                .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
                .collect();

            if tool_calls.is_empty() {
                // Final answer turn: publish its tokens.
                let answer = resp.text();
                if deltas.is_empty() && !answer.is_empty() {
                    Self::publish_deltas(runtime, &prep.row_id, std::slice::from_ref(&answer));
                } else {
                    Self::publish_deltas(runtime, &prep.row_id, &deltas);
                }
                return Ok(answer);
            }

            // Intermediate turn: drop its buffered text, execute the tools,
            // append the assistant turn and the tool results, and continue.
            messages.push(LlmMessage {
                role: MessageRole::Assistant,
                content: resp.content.clone(),
            });
            let mut results = Vec::with_capacity(tool_calls.len());
            for (id, name, input) in &tool_calls {
                let (content, is_error) = self.execute_tool(name, input, &prep.member_ids).await;
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
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

        // The final turn sets `force_answer` (no tools), so the loop always
        // returns from the `tool_calls.is_empty()` branch above.
        unreachable!("agent loop must answer on its final, tool-free turn")
    }

    /// Publish a sequence of answer-token deltas onto the chain broadcaster.
    fn publish_deltas(runtime: &Arc<ChainRuntime>, qa_id: &str, deltas: &[String]) {
        for delta in deltas {
            runtime.publish(ChainSseEvent::Token {
                chain_qa_id: qa_id.to_string(),
                delta: delta.clone(),
            });
        }
    }

    /// Execute one tool call. Read-only: `search_conversations` runs ranked
    /// retrieval scoped to the chain's members; `read_conversation` returns a
    /// byte-budgeted page of one member's full transcript. Both are bound to
    /// the chain — a read outside the member set is refused (REQ-RET-008).
    /// Returns `(content, is_error)` for the tool result.
    async fn execute_tool(
        &self,
        name: &str,
        input: &serde_json::Value,
        member_ids: &[String],
    ) -> (String, bool) {
        match name {
            "search_conversations" => {
                let query = input
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim();
                if query.is_empty() {
                    return (
                        "error: search_conversations requires a non-empty 'query'".to_string(),
                        true,
                    );
                }
                match self
                    .retriever
                    .retrieve(
                        query,
                        RetrievalScope::Conversations(member_ids.to_vec()),
                        SEARCH_TOP_K,
                    )
                    .await
                {
                    Ok(hits) if hits.is_empty() => (
                        "No matching messages found in this chain.".to_string(),
                        false,
                    ),
                    Ok(hits) => (format_search_hits(&hits), false),
                    Err(e) => (format!("error: search failed: {e}"), true),
                }
            }
            "read_conversation" => {
                let conv_id = input
                    .get("conversation_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if conv_id.is_empty() {
                    return (
                        "error: read_conversation requires 'conversation_id'".to_string(),
                        true,
                    );
                }
                if !member_ids.iter().any(|m| m == conv_id) {
                    return (
                        format!("error: conversation {conv_id} is not part of this chain"),
                        true,
                    );
                }
                let cursor = usize::try_from(
                    input
                        .get("cursor")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                )
                .unwrap_or(0);
                match self.db.get_messages(conv_id).await {
                    Ok(msgs) => (read_page(&msgs, cursor), false),
                    Err(e) => (format!("error: read failed: {e}"), true),
                }
            }
            other => (format!("error: unknown tool '{other}'"), true),
        }
    }

    /// Terminal status transition. Updates the persisted `chain_qa` row to
    /// `completed` / `failed` and publishes the matching terminal event so
    /// live subscribers can clear their streaming UI state.
    async fn finalize(
        &self,
        qa_id: &str,
        result: Result<String, RunInvocationError>,
        runtime: &Arc<ChainRuntime>,
    ) {
        match result {
            Ok(answer) => {
                if let Err(e) = self.db.complete_chain_qa(qa_id, &answer, Utc::now()).await {
                    tracing::error!(
                        qa_id = %qa_id, error = %e,
                        "chain Q&A complete UPDATE failed; row will be swept on restart",
                    );
                }
                runtime.publish(ChainSseEvent::Completed {
                    chain_qa_id: qa_id.to_string(),
                    full_answer: answer,
                });
            }
            Err(RunInvocationError {
                error,
                partial_answer,
            }) => {
                tracing::warn!(qa_id = %qa_id, error = %error, "chain Q&A invocation failed");
                if let Err(e) = self
                    .db
                    .fail_chain_qa(qa_id, partial_answer.as_deref())
                    .await
                {
                    tracing::error!(
                        qa_id = %qa_id, error = %e,
                        "chain Q&A fail UPDATE failed; row will be swept on restart",
                    );
                }
                runtime.publish(ChainSseEvent::Failed {
                    chain_qa_id: qa_id.to_string(),
                    error: error.to_string(),
                    partial_answer,
                });
            }
        }
    }

    /// Read-side: fetch persisted Q&A history for a chain (REQ-CHN-005).
    pub async fn list_history(&self, root_id: &str) -> Result<Vec<ChainQaRow>, ChainQaError> {
        Ok(self.db.list_chain_qa(root_id).await?)
    }
}

/// Per-submission state passed from `prepare_invocation` to
/// `run_answer_invocation` and `finalize`.
struct PreparedInvocation {
    row_id: ChainQaId,
    question: String,
    /// Orientation skeleton (member ids + titles + continuation summaries).
    skeleton: String,
    /// The chain's member conversation ids — the tool scope (REQ-RET-008).
    member_ids: Vec<String>,
    service: Arc<dyn LlmService>,
    #[allow(dead_code)] // Persisted into chain_qa.model via insert_chain_qa.
    model_id: String,
    /// Language inherited from the chain's root conversation.
    language: crate::llm_language::LlmLanguage,
}

/// Internal error wrapper pairing a [`ChainQaError`] with whatever partial
/// answer streamed before the failure (so `finalize` can persist the partial
/// into `chain_qa.answer` per REQ-CHN-005).
struct RunInvocationError {
    error: ChainQaError,
    partial_answer: Option<String>,
}

/// Build one turn's `LlmRequest`. Offers the two Q&A tools unless
/// `force_answer` (the final allowed turn), where an empty tool set forces the
/// model to answer with what it has.
fn build_agent_request(
    messages: &[LlmMessage],
    language: crate::llm_language::LlmLanguage,
    force_answer: bool,
) -> LlmRequest {
    let tools = if force_answer { vec![] } else { qa_tools() };
    LlmRequest {
        system: vec![SystemContent::new(
            crate::llm_language::chain_qa_agent_system_prompt(language),
        )],
        messages: messages.to_vec(),
        tools,
        max_tokens: Some(ANSWER_MAX_TOKENS),
        // One cache key per language so phoenix-native and caveman prompts
        // don't collide on a shared cache slot.
        cache_key: PromptCacheKey::stable(format!("chain-qa-agent/{}", language.as_str())),
    }
}

/// The two read-only, scope-bound tools the Q&A agent drives (REQ-CHN-009).
/// The scope is fixed by the host (the chain's members); the model supplies
/// only the query / target — it cannot widen its reach (REQ-RET-008).
fn qa_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_conversations".to_string(),
            description: "Search this chain's messages by relevance to a natural-language query. \
                Returns ranked snippets, each tagged with its source conversation id. Use this to \
                locate where something was discussed, then read that conversation in full."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "A natural-language search query (e.g. 'rate limiter token bucket')."
                    }
                },
                "required": ["query"]
            }),
            defer_loading: false,
        },
        ToolDefinition {
            name: "read_conversation".to_string(),
            description: "Read the full content of one chain member — including complete tool \
                output — one bounded page at a time. Pass a conversation_id from the skeleton or a \
                search result. If the page ends with a 'more' marker, call again with the given \
                cursor to continue."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "conversation_id": {
                        "type": "string",
                        "description": "A conversation id from the chain skeleton or a search result."
                    },
                    "cursor": {
                        "type": "integer",
                        "description": "Resume offset returned by a previous read_conversation page; omit to start at the beginning."
                    }
                },
                "required": ["conversation_id"]
            }),
            defer_loading: false,
        },
    ]
}

/// Format ranked search hits into a compact, citable tool result.
fn format_search_hits(hits: &[RetrievedChunk]) -> String {
    let mut out = String::new();
    for hit in hits {
        let _ = writeln!(
            out,
            "[#{} · {} · {}] {}",
            hit.conversation_id,
            hit.message_type,
            hit.created_at.format("%Y-%m-%d"),
            hit.snippet.trim()
        );
    }
    out
}

/// Return one host-budgeted page of a conversation's full transcript starting
/// at character `cursor`. The budget is fixed by the host ([`READ_PAGE_CHARS`],
/// REQ-RET-008); paging by character offset bounds the page even within one
/// oversized message.
fn read_page(messages: &[Message], cursor: usize) -> String {
    let full: Vec<char> = render_full_transcript(messages).chars().collect();
    let total = full.len();
    if cursor >= total {
        return "(end of conversation)".to_string();
    }
    let end = cursor.saturating_add(READ_PAGE_CHARS).min(total);
    let page: String = full[cursor..end].iter().collect();
    if end < total {
        format!("{page}\n[… more content; call read_conversation again with cursor={end}]")
    } else {
        page
    }
}

/// Render a conversation transcript with **full** content, including complete
/// tool-result bodies (unlike the search index, which keeps a head+tail
/// excerpt for ranking). This is the agent's ground-truth read path.
fn render_full_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let label = match m.message_type {
            MessageType::User => "User",
            MessageType::Agent => "Agent",
            MessageType::Tool => "Tool",
            MessageType::System => "System",
            MessageType::Error => "Error",
            MessageType::Continuation => "Continuation",
            MessageType::Skill => "Skill",
        };
        let body = match &m.content {
            MessageContent::User(c) => c.text.clone(),
            MessageContent::Agent(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageContent::Tool(c) => c.content.clone(),
            MessageContent::System(c) => c.text.clone(),
            MessageContent::Error(c) => c.message.clone(),
            MessageContent::Continuation(c) => c.summary.clone(),
            MessageContent::Skill(c) => format!("/{} {}", c.name, c.trigger),
        };
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&body);
        out.push('\n');
    }
    out
}

/// Find the **trailing** `MessageType::Continuation` message and extract its
/// summary. Returns None when the conversation has no Continuation message
/// (degenerate non-leaf state).
fn trailing_continuation_summary(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| match &m.content {
        MessageContent::Continuation(c) => Some(c.summary.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests;
