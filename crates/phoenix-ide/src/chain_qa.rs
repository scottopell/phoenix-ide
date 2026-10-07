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
    NewChainQa, RetrievalRequest, RetrievalScope, RetrievedChunk,
};
use chrono::Utc;
use phoenix_llm::{
    ContentBlock, LlmError, LlmMessage, LlmRequest, LlmService, MessageRole, ModelRegistry,
    PromptCacheKey, SystemContent, TokenChunk, ToolDefinition,
};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Maximum provider requests per Q&A invocation, including final answer retries.
const MAX_QA_TURNS: usize = 6;

/// Maximum tool calls actually executed in one planning turn. Providers may
/// batch parallel tool calls in a single assistant message; without a cap a
/// batch of `read_conversation`s could inject (batch × [`READ_PAGE_CHARS`])
/// into the next turn, defeating the per-page budget. Calls beyond the cap get
/// a "skipped" tool result (every `tool_use` must still be answered to keep the
/// request valid) so the model re-requests fewer.
const MAX_TOOL_CALLS_PER_TURN: usize = 4;

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

/// Block-wait budget for the chain's index to become fresh before answering
/// (REQ-CHN-009): up to `INDEX_WAIT_ATTEMPTS * INDEX_POLL_MS` ms. After that we
/// answer anyway (`read_conversation` reads live `messages`, so the agent can
/// still answer — only search coverage may lag). The reconcile sweep is fast,
/// so this only ever bites in the first moments after startup.
const INDEX_WAIT_ATTEMPTS: usize = 100;
const INDEX_POLL_MS: u64 = 100;

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
        admission_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
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
            let _admission_guard = admission_guard;
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
        let effective_effort = self.llm_registry.effective_effort(&model_id, None);
        let max_output_tokens = self.llm_registry.output_token_limit(&model_id);

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
                chain_members_at_answer: snapshot.member_count,
                chain_messages_at_answer: snapshot.total_messages,
                created_at,
            })
            .await?;

        Ok(PreparedInvocation {
            row_id: qa_id,
            question: question.to_string(),
            skeleton,
            root_id: root_id.to_string(),
            snapshot,
            service,
            model_id,
            language,
            effective_effort,
            max_output_tokens,
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
    /// First blocks until the chain's index is fresh (so search sees the whole
    /// chain), then runs **planning turns** that may call the scope-bound
    /// search/read tools. Planning turns run non-streamed, so intermediate
    /// "I'll search…" narration never reaches the user. When the model stops
    /// calling tools (or the turn cap is hit), a dedicated final turn with new
    /// tool calls disabled streams the answer over the chain broadcaster.
    #[allow(clippy::too_many_lines)] // bounded agent loop keeps replay/tool sequencing visible
    async fn run_answer_invocation(
        &self,
        prep: &PreparedInvocation,
        runtime: &Arc<ChainRuntime>,
    ) -> Result<AnswerOutcome, RunInvocationError> {
        // Block-wait for the chain's index to catch up before answering, so the
        // agent doesn't search a partial index right after startup (REQ-CHN-009).
        // If the wait budget elapses while the index is still not fresh, we
        // cannot rule out stale/orphaned rows that ranked search would surface
        // as deleted/edited-away content — so `search_conversations` is withheld
        // (only `read_conversation`, which reads live `messages`, is offered) and
        // the agent is told to read members directly (REQ-RET: deleted content
        // is never returned).
        let index_fresh = self.await_index_fresh(&prep.root_id).await;
        let coverage_note = if index_fresh {
            ""
        } else {
            "\n\nNote: search is unavailable for this question because the index \
             is not yet confirmed up to date. Read the relevant members directly \
             with read_conversation to answer."
        };

        let mut provider_replay: Option<
            phoenix_core::domain::provider_replay::AnthropicReplayPayload,
        > = None;
        let mut responses_replay = Vec::new();
        let mut messages = vec![LlmMessage {
            source_message_id: Some("chain-qa-question".into()),
            role: MessageRole::User,
            content: vec![ContentBlock::text(format!(
                "Conversation transcripts (in order):\n{}\n---\nQuestion: {}{}",
                prep.skeleton, prep.question, coverage_note
            ))],
        }];

        let mut planning_requests = 0;
        for turn in 0..MAX_QA_TURNS {
            // Final allowed turn: answer directly (streamed, no tools).
            if turn + 1 == MAX_QA_TURNS {
                break;
            }

            // Planning turn: offer tools, non-streamed so its (possibly
            // narrated) text never reaches the user.
            let request = build_agent_request(
                &messages,
                prep.language,
                false,
                index_fresh,
                prep.effective_effort,
                prep.max_output_tokens,
            );
            let mut request = request;
            request.provider_replay = provider_replay.clone();
            request.responses_replay = responses_replay.clone();
            planning_requests += 1;
            let resp = prep
                .service
                .complete(&request)
                .await
                .map_err(|e| RunInvocationError {
                    error: ChainQaError::from(e),
                    partial_answer: None,
                })?;

            let tool_calls: Vec<(String, String, serde_json::Value)> = resp
                .tool_uses()
                .into_iter()
                .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
                .collect();

            // No tool call → the agent is ready; stream the final answer.
            if tool_calls.is_empty() {
                break;
            }

            adopt_qa_replay(
                &mut request,
                resp.provider_replay.clone(),
                format!("chain-qa-{turn}"),
            )
            .map_err(|error| RunInvocationError {
                error: ChainQaError::Llm(error),
                partial_answer: None,
            })?;
            provider_replay = request.provider_replay.take();
            responses_replay = std::mem::take(&mut request.responses_replay);

            // Execute the tools and feed results back. Re-resolve the chain's
            // members live so a continuation added mid-run is in scope
            // (REQ-RET-008 host-bound-to-root, REQ-CHN-009). A lookup failure
            // here would silently empty the scope (every search a miss, every
            // read "not part of this chain"), so fail the Q&A instead of
            // answering against a fabricated empty chain.
            let members = self
                .db
                .chain_members_forward(&prep.root_id)
                .await
                .map_err(|e| RunInvocationError {
                    error: ChainQaError::from(e),
                    partial_answer: None,
                })?;
            messages.push(LlmMessage {
                source_message_id: Some(format!("chain-qa-{turn}")),
                role: MessageRole::Assistant,
                content: resp.content.clone(),
            });
            let mut results = Vec::with_capacity(tool_calls.len());
            for (idx, (id, name, input)) in tool_calls.iter().enumerate() {
                // Cap the executed calls so a batched response can't blow past
                // the per-page budget; still answer every tool_use so the next
                // request stays valid.
                let (content, is_error) = if !request.tool_availability.is_callable(name) {
                    (format!("EUNAVAIL: tool {name} is unavailable"), true)
                } else if idx < MAX_TOOL_CALLS_PER_TURN {
                    self.execute_tool(name, input, &members).await
                } else {
                    (
                        format!(
                            "error: too many tool calls in one turn (limit {MAX_TOOL_CALLS_PER_TURN}); \
                             this call was not run — issue fewer calls per turn"
                        ),
                        true,
                    )
                };
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    images: vec![],
                    is_error,
                });
            }
            messages.push(LlmMessage {
                source_message_id: Some(format!("chain-qa-results-{turn}")),
                role: MessageRole::User,
                content: results,
            });
        }

        // Recompute the freshness snapshot from the chain's live shape *before*
        // the final request runs: this is the latest chain state the answer can
        // reflect (planning resolved members live; the final turn sees only
        // what's already in `messages`). Capturing it here — not after the
        // stream returns — means a write that lands while the answer is
        // generating is correctly counted as newer than the answer, so the UI
        // shows the age-of-answer tag. Falls back to the submission-time
        // snapshot on a DB error.
        let snapshot = self
            .live_snapshot(&prep.root_id)
            .await
            .unwrap_or(prep.snapshot);
        let answer = self
            .stream_final_answer(
                &messages,
                provider_replay,
                responses_replay,
                index_fresh,
                MAX_QA_TURNS - planning_requests,
                prep,
                runtime,
            )
            .await?;
        Ok(AnswerOutcome { answer, snapshot })
    }

    /// Recompute the chain snapshot from its current members. Returns `None` on
    /// any DB error so the caller can fall back to the submission-time value
    /// rather than fail an otherwise-successful answer.
    async fn live_snapshot(&self, root_id: &str) -> Option<ChainSnapshot> {
        let member_ids = self.db.chain_members_forward(root_id).await.ok()?;
        let mut members = Vec::with_capacity(member_ids.len());
        for id in &member_ids {
            members.push(self.db.get_conversation(id).await.ok()?);
        }
        Some(compute_chain_snapshot(&members))
    }

    /// Stream the final answer phase onto the chain broadcaster (REQ-CHN-004).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn stream_final_answer(
        &self,
        messages: &[LlmMessage],
        provider_replay: Option<phoenix_core::domain::provider_replay::AnthropicReplayPayload>,
        responses_replay: Vec<phoenix_core::domain::responses_replay::ResponsesResponseSet>,
        search_enabled: bool,
        remaining_turns: usize,
        prep: &PreparedInvocation,
        runtime: &Arc<ChainRuntime>,
    ) -> Result<String, RunInvocationError> {
        let request = build_agent_request(
            messages,
            prep.language,
            true,
            search_enabled,
            prep.effective_effort,
            prep.max_output_tokens,
        );
        let mut request = request;
        request.provider_replay = provider_replay;
        request.responses_replay = responses_replay;
        let mut partial = String::new();
        for turn in 0..remaining_turns {
            let (chunk_tx, mut chunk_rx) = mpsc::channel::<TokenChunk>(256);
            let qa_id = prep.row_id.clone();
            let runtime_handle = Arc::clone(runtime);
            let forwarder = tokio::spawn(async move {
                let mut partial = String::new();
                while let Some(chunk) = chunk_rx.recv().await {
                    if let TokenChunk::Text(delta) = chunk {
                        partial.push_str(&delta);
                        runtime_handle.publish(ChainSseEvent::Token {
                            chain_qa_id: qa_id.clone(),
                            delta,
                        });
                    }
                }
                partial
            });
            let response = prep.service.complete_streaming(&request, &chunk_tx).await;
            drop(chunk_tx);
            partial.push_str(&forwarder.await.unwrap_or_default());
            let response = response.map_err(|error| RunInvocationError {
                error: ChainQaError::from(error),
                partial_answer: (!partial.is_empty()).then(|| partial.clone()),
            })?;
            let tool_calls = response.tool_uses();
            if tool_calls.is_empty() {
                return if response.end_turn {
                    Ok(response.text())
                } else {
                    Err(RunInvocationError {
                        error: ChainQaError::Llm("final answer response did not terminate".into()),
                        partial_answer: (!partial.is_empty()).then_some(partial),
                    })
                };
            }
            let owner_id = format!("chain-qa-final-{turn}");
            adopt_qa_replay(
                &mut request,
                response.provider_replay.clone(),
                owner_id.clone(),
            )
            .map_err(|error| RunInvocationError {
                error: ChainQaError::Llm(error),
                partial_answer: (!partial.is_empty()).then(|| partial.clone()),
            })?;
            let results = tool_calls
                .into_iter()
                .map(|(id, name, _)| ContentBlock::ToolResult {
                    tool_use_id: id.to_owned(),
                    content: format!("EUNAVAIL: tool {name} is unavailable; answer without tools"),
                    images: Vec::new(),
                    is_error: true,
                })
                .collect();
            request.messages.push(LlmMessage {
                source_message_id: Some(owner_id),
                role: MessageRole::Assistant,
                content: response.content,
            });
            request.messages.push(LlmMessage {
                source_message_id: Some(format!("chain-qa-final-results-{turn}")),
                role: MessageRole::User,
                content: results,
            });
        }
        Err(RunInvocationError {
            error: ChainQaError::Llm("Q&A request limit reached before a terminal answer".into()),
            partial_answer: (!partial.is_empty()).then_some(partial),
        })
    }

    /// Block until the chain's index is fresh for its members, or the wait
    /// budget elapses (REQ-CHN-009). Returns whether the index is fresh:
    /// `false` means the caller should warn the agent that search coverage may
    /// be partial. Best-effort — on a DB/retriever error it returns `false`
    /// (treat as partial) and the caller answers anyway, since
    /// `read_conversation` reads live `messages` regardless of the index.
    async fn await_index_fresh(&self, root_id: &str) -> bool {
        let Ok(members) = self.db.chain_members_forward(root_id).await else {
            return false;
        };
        if members.is_empty() {
            return true;
        }
        for _ in 0..INDEX_WAIT_ATTEMPTS {
            match self.retriever.is_fresh_for(&members).await {
                Ok(true) => return true,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(root = %root_id, error = %e, "chain index freshness check failed; answering on a partial index");
                    return false;
                }
            }
            tokio::time::sleep(Duration::from_millis(INDEX_POLL_MS)).await;
        }
        tracing::warn!(root = %root_id, "chain index not fresh after wait; answering on a partial index");
        false
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
                // Re-validate freshness against the *live* member set at call
                // time: the scope is re-resolved each turn, so a continuation
                // added since the warmup wait could carry an unconfirmed/stale
                // FTS row. Refuse the search rather than risk returning
                // deleted/edited-away content (REQ-RET); the agent can still
                // read members directly.
                match self.retriever.is_fresh_for(member_ids).await {
                    Ok(true) => {}
                    Ok(false) => {
                        return (
                            "search unavailable: the index is not current for this chain \
                             (it may have just grown); read members directly with read_conversation"
                                .to_string(),
                            true,
                        );
                    }
                    Err(e) => return (format!("error: index freshness check failed: {e}"), true),
                }
                match self
                    .retriever
                    .retrieve(RetrievalRequest::natural_language(
                        query,
                        RetrievalScope::Conversations(member_ids.to_vec()),
                        SEARCH_TOP_K,
                    ))
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
                // The skeleton and search hits render ids with a leading `#`
                // (`#<id>`), and the tool tells the model to pass an id "from
                // the skeleton or a search result" — so accept that exact token
                // by stripping a leading `#` before the membership check.
                let conv_id = input
                    .get("conversation_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim();
                let conv_id = conv_id.strip_prefix('#').unwrap_or(conv_id);
                if conv_id.is_empty() {
                    return (
                        "error: read_conversation requires 'conversation_id'".to_string(),
                        true,
                    );
                }
                if !member_ids.iter().any(|m| m == conv_id) {
                    return (
                        format!(
                            "error: transcript {conv_id} is not part of this ProductConversation"
                        ),
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
        result: Result<AnswerOutcome, RunInvocationError>,
        runtime: &Arc<ChainRuntime>,
    ) {
        match result {
            Ok(AnswerOutcome { answer, snapshot }) => {
                if let Err(e) = self
                    .db
                    .complete_chain_qa(
                        qa_id,
                        &answer,
                        snapshot.member_count,
                        snapshot.total_messages,
                        Utc::now(),
                    )
                    .await
                {
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
    /// The chain root. The tool scope is re-resolved live from this per turn
    /// (`chain_members_forward`), so a continuation added mid-run is in scope
    /// (REQ-RET-008 host-bound-to-root, REQ-CHN-009).
    root_id: String,
    /// Chain shape at submission time. Used as the fallback freshness snapshot
    /// if the completion-time recompute fails — never staler than the row's
    /// inserted value.
    snapshot: ChainSnapshot,
    service: Arc<dyn LlmService>,
    #[allow(dead_code)] // Persisted into chain_qa.model via insert_chain_qa.
    model_id: String,
    effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
    max_output_tokens: Option<u32>,
    /// Language inherited from the chain's root conversation.
    language: crate::llm_language::LlmLanguage,
}

/// Successful outcome of an agent run: the answer plus the chain snapshot as
/// of completion. The snapshot is recomputed at completion (not reused from
/// submission) because the tool scope resolves chain members live, so a
/// continuation added mid-run can legitimately inform the answer — the
/// persisted freshness counters must reflect the shape the answer actually saw
/// (REQ-CHN-005, REQ-CHN-009).
struct AnswerOutcome {
    answer: String,
    snapshot: ChainSnapshot,
}

/// Internal error wrapper pairing a [`ChainQaError`] with whatever partial
/// answer streamed before the failure (so `finalize` can persist the partial
/// into `chain_qa.answer` per REQ-CHN-005).
struct RunInvocationError {
    error: ChainQaError,
    partial_answer: Option<String>,
}

/// Build a Q&A request with retained declarations and current execution permission.
fn build_agent_request(
    messages: &[LlmMessage],
    language: crate::llm_language::LlmLanguage,
    force_answer: bool,
    search_enabled: bool,
    effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
    max_output_tokens: Option<u32>,
) -> LlmRequest {
    let tools = qa_tools(search_enabled);
    let tool_availability = if force_answer {
        use phoenix_core::domain::tool_availability::{
            PositionedToolChange, ToolAvailability, ToolChange,
        };
        let changes = messages
            .last()
            .and_then(|message| message.source_message_id.as_ref())
            .map_or_else(Vec::new, |anchor| {
                tools
                    .iter()
                    .map(|tool| PositionedToolChange {
                        after_message_id: anchor.clone(),
                        change: ToolChange::Removal {
                            name: tool.name.clone(),
                        },
                    })
                    .collect()
            });
        ToolAvailability::new(tools.clone(), std::collections::BTreeSet::new())
            .expect("QA tool declarations are unique")
            .with_anthropic_context(tools, changes)
            .expect("QA withdrawals reference retained declarations")
    } else {
        phoenix_core::domain::tool_availability::ToolAvailability::all(tools)
    };
    LlmRequest {
        system: vec![SystemContent::new(
            crate::llm_language::chain_qa_agent_system_prompt(language),
        )],
        messages: messages.to_vec(),
        provider_replay: None,
        responses_replay: Vec::new(),
        tool_availability,
        max_tokens: Some(
            max_output_tokens.map_or(ANSWER_MAX_TOKENS, |limit| limit.min(ANSWER_MAX_TOKENS)),
        ),
        effective_effort,
        service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
        telemetry: None,
        // One cache key per language so phoenix-native and caveman prompts
        // don't collide on a shared cache slot.
        cache_key: PromptCacheKey::stable(format!("chain-qa-agent/{}", language.as_str())),
    }
}

/// The read-only, scope-bound tools the Q&A agent drives (REQ-CHN-009). The
/// scope is fixed by the host (the chain's members); the model supplies only
/// the query / target — it cannot widen its reach (REQ-RET-008).
///
/// `search_conversations` is offered only when the index is fresh: a stale or
/// still-pruning index could otherwise surface deleted/edited-away content
/// through ranked search, which `read_conversation` (live `messages`) never
/// can — so when freshness can't be established the agent gets the read tool
/// alone (REQ-RET: deleted content is never returned).
fn qa_tools(search_enabled: bool) -> Vec<ToolDefinition> {
    let mut tools = Vec::with_capacity(2);
    if search_enabled {
        tools.push(ToolDefinition {
            name: "search_conversations".to_string(),
            description: "Search this ProductConversation's transcript messages by relevance to a natural-language query. \
                Returns ranked snippets tagged with their exact source transcript id. Use this to \
                locate where something was discussed, then read that transcript."
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
        });
    }
    tools.push(ToolDefinition {
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
                    "description": "An exact transcript id from the transcript list or a search result."
                },
                "cursor": {
                    "type": "integer",
                    "description": "Resume offset returned by a previous read_conversation page; omit to start at the beginning."
                }
            },
            "required": ["conversation_id"]
        }),
        defer_loading: false,
    });
    tools
}

/// Format ranked search hits into a compact, citable tool result.
fn format_search_hits(hits: &[RetrievedChunk]) -> String {
    let mut out = String::new();
    for hit in hits {
        let _ = writeln!(
            out,
            "[#{} · {}{} · {}] {}",
            hit.conversation_id,
            crate::api::global_read::attributed_role(hit.message_type, &hit.origin),
            crate::api::global_read::attributed_sender(&hit.origin),
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
    let end = cursor.saturating_add(READ_PAGE_CHARS);
    let mut out = String::new();
    let mut pos = 0usize; // char offset into the full (non-hidden) transcript
    let mut has_more = false;
    'outer: for m in messages {
        if message_is_hidden(m) {
            continue;
        }
        // Render one message at a time and copy only the window chars, so a
        // multi-message transcript is never fully materialized; stop as soon as
        // the page is filled (the next char proves there's more).
        for ch in render_message_line(m).chars() {
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
    // `pos` is now the total transcript length (unless we stopped early).
    if out.is_empty() && !has_more {
        return "(end of transcript)".to_string();
    }
    if has_more {
        format!("{out}\n[… more content; call read_conversation again with cursor={end}]")
    } else {
        out
    }
}

/// Render a whole conversation transcript by concatenating
/// [`render_message_line`] over non-hidden messages. The production read path
/// ([`read_page`]) streams the window incrementally instead; this materializes
/// the full transcript and is used only to assert rendering in tests.
#[cfg(test)]
fn render_full_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        // Mirror the index extractor: never surface UI-hidden recovery/
        // dismissal markers, so the agent can't answer from suppressed
        // implementation artifacts.
        if message_is_hidden(m) {
            continue;
        }
        out.push_str(&render_message_line(m));
    }
    out
}

/// Render one (non-hidden) message as a `"Label: body\n"` line for the read
/// path. Factored out so [`read_page`] can stream the transcript a message at a
/// time without materializing the whole thing.
fn render_message_line(m: &Message) -> String {
    let label = format!(
        "{}{}",
        crate::api::global_read::attributed_role(m.message_type, &m.origin),
        crate::api::global_read::attributed_sender(&m.origin)
    );
    let body = match &m.content {
        // `llm_text()` is the expanded form the model actually saw (e.g.
        // @file content), not the display shorthand. Attached images aren't
        // representable in the text read path — surface the gap rather than
        // presenting an apparently complete message.
        MessageContent::User(c) => {
            let mut text = c.llm_text().to_string();
            // Non-image attachments contribute their context tag (name /
            // path / metadata) to LLM history at runtime; include it so the
            // agent can answer about attached files it reads.
            for f in &c.files {
                text.push('\n');
                text.push_str(&f.llm_context_tag());
            }
            if !c.images.is_empty() {
                tracing::debug!(
                        n = c.images.len(),
                        "chain Q&A read_conversation: dropping user-message images — image recall is unsupported",
                    );
                let _ = write!(
                        text,
                        "\n[{} image(s) attached to this message are not shown — chain Q&A reads text only]",
                        c.images.len()
                    );
            }
            text
        }
        // Keep text AND a readable rendering of every tool block (local,
        // server-side, and MCP tool calls/results) — recall questions about
        // what was searched, fetched, run, or returned live in those blocks.
        MessageContent::Agent(blocks) => blocks
            .iter()
            .map(ContentBlock::render_text)
            .collect::<Vec<_>>()
            .join("\n"),
        // Keep the full tool-result text. Tool results can also carry image
        // payloads bound for the LLM, but the chain Q&A read path is
        // text-only — surface the gap to the agent (and the logs) rather
        // than silently stranding the images.
        MessageContent::Tool(c) => {
            if c.images.is_empty() {
                c.content.clone()
            } else {
                tracing::debug!(
                    tool_use_id = %c.tool_use_id,
                    n = c.images.len(),
                    "chain Q&A read_conversation: dropping tool-result images — image recall is unsupported",
                );
                format!(
                        "{}\n[{} image(s) in this tool result are not shown — chain Q&A reads text only]",
                        c.content,
                        c.images.len()
                    )
            }
        }
        MessageContent::System(c) => c.text.clone(),
        MessageContent::Error(c) => c.message.clone(),
        MessageContent::Continuation(c) => c.summary.clone(),
        // Include the expanded skill body (and any attached file tags), not
        // just the trigger — when a question depends on instructions a skill
        // injected, that text lives in `body`.
        MessageContent::Skill(c) => {
            let mut body = format!("/{} {}\n{}", c.name, c.trigger, c.body);
            for f in &c.files {
                body.push('\n');
                body.push_str(&f.llm_context_tag());
            }
            body
        }
    };
    format!("{label}: {body}\n")
}

/// Whether the UI hides this message (`display_data.hidden == true`) — e.g.
/// dismissed-error/question recovery markers. Mirrors the index extractor's
/// hidden guard so the read path and the search index agree on what content
/// is user-visible.
fn message_is_hidden(m: &Message) -> bool {
    m.display_data
        .as_ref()
        .and_then(|d| d.get("hidden"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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

fn adopt_qa_replay(
    request: &mut LlmRequest,
    update: Option<phoenix_core::domain::provider_replay::ProviderReplayUpdate>,
    owner_id: String,
) -> Result<(), String> {
    use phoenix_core::domain::provider_replay::{AnthropicReplayPayload, ProviderReplayUpdate};
    match update {
        Some(ProviderReplayUpdate::Anthropic(response)) => {
            let mut sets = request
                .provider_replay
                .take()
                .map_or_else(Vec::new, |p| p.response_sets);
            sets.push(response.with_owner_message_id(owner_id));
            request.provider_replay =
                Some(AnthropicReplayPayload::new(sets).map_err(|e| e.to_string())?);
        }
        Some(ProviderReplayUpdate::Responses(response)) => {
            request
                .responses_replay
                .push(response.with_owner_message_id(owner_id));
        }
        Some(ProviderReplayUpdate::Clear) => {
            request.provider_replay = None;
            request.responses_replay.clear();
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tool_policy_tests {
    use super::*;

    #[derive(Debug)]
    struct ResponsesQaLlm {
        rounds: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmService for ResponsesQaLlm {
        async fn complete(
            &self,
            request: &LlmRequest,
        ) -> Result<phoenix_llm::LlmResponse, LlmError> {
            use phoenix_core::domain::{
                provider_replay::ProviderReplayUpdate, responses_replay::ResponsesResponseSet,
            };
            let round = self
                .rounds
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(request.responses_replay.len(), round);
            for set in &request.responses_replay {
                let owner = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.source_message_id.as_deref() == Some(set.owner_message_id.as_str())
                    })
                    .unwrap();
                assert_eq!(owner.content, set.public_content);
            }
            let mut response = phoenix_llm::LlmResponse::non_streaming(
                vec![ContentBlock::text("ready")],
                true,
                phoenix_llm::Usage::default(),
            );
            response.provider_replay = Some(ProviderReplayUpdate::Clear);
            if round < 2 {
                let call_id = format!("read-{round}");
                response.content = vec![ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: "read_conversation".into(),
                    input: serde_json::json!({"conversation_id":"qa-replay-root"}),
                }];
                response.end_turn = false;
                response.provider_replay = Some(ProviderReplayUpdate::Responses(
                    ResponsesResponseSet {
                        response_id: format!("response-{round}"),
                        model: "test-model".into(),
                        owner_message_id: String::new(),
                        public_content: response.content.clone(),
                        output_items: vec![
                            serde_json::json!({"type":"reasoning","id":format!("reasoning-{round}"),"encrypted_content":"private"}),
                            serde_json::json!({"type":"function_call","id":format!("function-{round}"),"call_id":call_id,"name":"read_conversation","arguments":serde_json::json!({"conversation_id":"qa-replay-root"}).to_string()}),
                        ],
                    },
                ));
            }
            Ok(response)
        }

        async fn complete_streaming(
            &self,
            request: &LlmRequest,
            tx: &mpsc::Sender<TokenChunk>,
        ) -> Result<phoenix_llm::LlmResponse, LlmError> {
            assert_eq!(
                request.responses_replay.len(),
                2,
                "discarded planning terminal must not settle adopted tool history"
            );
            assert!(request.tool_availability.callable_names().is_empty());
            assert!(!request.tool_availability.declarations().is_empty());
            for (round, set) in request.responses_replay.iter().enumerate() {
                assert_eq!(set.owner_message_id, format!("chain-qa-{round}"));
                assert_eq!(set.output_items[0]["encrypted_content"], "private");
                let owner = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.source_message_id.as_deref() == Some(set.owner_message_id.as_str())
                    })
                    .unwrap();
                assert_eq!(owner.content, set.public_content);
            }
            tx.send(TokenChunk::Text("final answer".into()))
                .await
                .unwrap();
            Ok(phoenix_llm::LlmResponse::non_streaming(
                vec![ContentBlock::text("final answer")],
                true,
                phoenix_llm::Usage::default(),
            ))
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "test-model"
        }
    }

    #[tokio::test]
    async fn qa_replays_two_responses_tool_rounds_into_final_streamed_answer() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("qa-replay-root", "qa-replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let llm = Arc::new(ResponsesQaLlm {
            rounds: std::sync::atomic::AtomicUsize::default(),
        });
        let registry = Arc::new(ModelRegistry::for_test_with_sonnet(llm.clone()));
        let qa = ChainQa::new(db.clone(), registry, Arc::new(db.fts_retriever()));
        let prep = PreparedInvocation {
            row_id: "qa-replay-row".into(),
            question: "What changed?".into(),
            skeleton: "#qa-replay-root".into(),
            root_id: "qa-replay-root".into(),
            snapshot: ChainSnapshot {
                member_count: 1,
                total_messages: 0,
            },
            service: llm.clone(),
            model_id: "test-model".into(),
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            max_output_tokens: None,
            language: crate::llm_language::LlmLanguage::PhoenixNative,
        };
        let runtime = qa.runtime_registry.get_or_create(&prep.root_id).await;
        let outcome = qa
            .run_answer_invocation(&prep, &runtime)
            .await
            .unwrap_or_else(|failure| panic!("QA failed: {}", failure.error));
        assert_eq!(outcome.answer, "final answer");
        assert_eq!(llm.rounds.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[derive(Debug)]
    struct DeniedFinalQaLlm {
        final_requests: std::sync::atomic::AtomicUsize,
        denied_rounds: usize,
        terminal: bool,
    }

    #[async_trait::async_trait]
    impl LlmService for DeniedFinalQaLlm {
        async fn complete(&self, _: &LlmRequest) -> Result<phoenix_llm::LlmResponse, LlmError> {
            Ok(phoenix_llm::LlmResponse::non_streaming(
                vec![ContentBlock::text("ready")],
                true,
                phoenix_llm::Usage::default(),
            ))
        }

        async fn complete_streaming(
            &self,
            request: &LlmRequest,
            tx: &mpsc::Sender<TokenChunk>,
        ) -> Result<phoenix_llm::LlmResponse, LlmError> {
            use phoenix_core::domain::{
                provider_replay::ProviderReplayUpdate, responses_replay::ResponsesResponseSet,
            };
            let turn = self
                .final_requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(request.tool_availability.callable_names().is_empty());
            assert!(request
                .tool_availability
                .anthropic_changes()
                .iter()
                .all(|change| change.after_message_id == "chain-qa-question"));
            assert_eq!(request.responses_replay.len(), turn);
            for prior in 0..turn {
                let set = &request.responses_replay[prior];
                assert_eq!(set.owner_message_id, format!("chain-qa-final-{prior}"));
                assert_eq!(set.output_items[0]["encrypted_content"], "private-final");
                let owner = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.source_message_id.as_deref() == Some(set.owner_message_id.as_str())
                    })
                    .unwrap();
                assert_eq!(owner.content, set.public_content);
                let results: Vec<_> = request
                    .messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .filter_map(|block| match block {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        } if tool_use_id == &format!("denied-{prior}") => Some((content, is_error)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(results.len(), 1);
                assert!(*results[0].1);
                assert_eq!(
                    results[0].0,
                    "EUNAVAIL: tool read_conversation is unavailable; answer without tools"
                );
            }
            if turn >= self.denied_rounds {
                tx.send(TokenChunk::Text("terminal answer".into()))
                    .await
                    .unwrap();
                return Ok(phoenix_llm::LlmResponse::non_streaming(
                    vec![ContentBlock::text("terminal answer")],
                    self.terminal,
                    phoenix_llm::Usage::default(),
                ));
            }
            tx.send(TokenChunk::Text("unfinished narration".into()))
                .await
                .unwrap();
            let content = vec![
                ContentBlock::text("unfinished narration"),
                ContentBlock::ToolUse {
                    id: format!("denied-{turn}"),
                    name: "read_conversation".into(),
                    input: serde_json::json!({"conversation_id":"qa-denied-root"}),
                },
            ];
            let mut response = phoenix_llm::LlmResponse::non_streaming(
                content.clone(),
                false,
                phoenix_llm::Usage::default(),
            );
            response.provider_replay =
                Some(ProviderReplayUpdate::Responses(ResponsesResponseSet {
                    response_id: format!("denied-response-{turn}"),
                    model: "test-model".into(),
                    owner_message_id: String::new(),
                    public_content: content,
                    output_items: vec![
                        serde_json::json!({"type":"reasoning","encrypted_content":"private-final"}),
                    ],
                }));
            Ok(response)
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "test-model"
        }
    }

    async fn run_denied_final(
        denied_rounds: usize,
        terminal: bool,
    ) -> (
        Result<AnswerOutcome, RunInvocationError>,
        Arc<DeniedFinalQaLlm>,
    ) {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("qa-denied-root", "qa-denied", "/tmp", true, None, None)
            .await
            .unwrap();
        let llm = Arc::new(DeniedFinalQaLlm {
            final_requests: std::sync::atomic::AtomicUsize::default(),
            denied_rounds,
            terminal,
        });
        let registry = Arc::new(ModelRegistry::for_test_with_sonnet(llm.clone()));
        let qa = ChainQa::new(db.clone(), registry, Arc::new(db.fts_retriever()));
        let prep = PreparedInvocation {
            row_id: "qa-denied-row".into(),
            question: "What changed?".into(),
            skeleton: "#qa-denied-root".into(),
            root_id: "qa-denied-root".into(),
            snapshot: ChainSnapshot {
                member_count: 1,
                total_messages: 0,
            },
            service: llm.clone(),
            model_id: "test-model".into(),
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            max_output_tokens: None,
            language: crate::llm_language::LlmLanguage::PhoenixNative,
        };
        let runtime = qa.runtime_registry.get_or_create(&prep.root_id).await;
        (qa.run_answer_invocation(&prep, &runtime).await, llm)
    }

    #[tokio::test]
    async fn final_tool_call_gets_one_unavailable_result_and_replay_before_terminal_answer() {
        let (outcome, llm) = run_denied_final(1, true).await;
        assert_eq!(
            outcome
                .unwrap_or_else(|failure| panic!("{}", failure.error))
                .answer,
            "terminal answer"
        );
        assert_eq!(
            llm.final_requests.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
    }

    #[tokio::test]
    async fn final_tool_calls_exhaust_existing_request_budget_without_false_success() {
        let (outcome, llm) = run_denied_final(usize::MAX, true).await;
        let Err(failure) = outcome else {
            panic!("tool calls cannot complete an answer")
        };
        assert!(failure.error.to_string().contains("request limit"));
        assert!(failure
            .partial_answer
            .unwrap()
            .contains("unfinished narration"));
        assert_eq!(
            llm.final_requests.load(std::sync::atomic::Ordering::SeqCst),
            MAX_QA_TURNS - 1
        );
    }

    #[tokio::test]
    async fn nonterminal_final_text_is_failed_with_partial_answer() {
        let (outcome, _) = run_denied_final(0, false).await;
        let Err(failure) = outcome else {
            panic!("nonterminal text cannot complete an answer")
        };
        assert!(failure.error.to_string().contains("did not terminate"));
        assert_eq!(failure.partial_answer.as_deref(), Some("terminal answer"));
    }

    #[test]
    fn forced_answer_retains_planning_declarations_and_withdraws_new_calls() {
        let messages = vec![
            LlmMessage {
                source_message_id: Some("chain-qa-question".into()),
                role: MessageRole::User,
                content: vec![ContentBlock::text("What changed?")],
            },
            LlmMessage {
                source_message_id: Some("chain-qa-1".into()),
                role: MessageRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "read-1".into(),
                    name: "read_conversation".into(),
                    input: serde_json::json!({"conversation_id":"root"}),
                }],
            },
            LlmMessage {
                source_message_id: Some("chain-qa-results-1".into()),
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "read-1".into(),
                    content: "transcript".into(),
                    images: vec![],
                    is_error: false,
                }],
            },
        ];
        for search_enabled in [false, true] {
            let planning = build_agent_request(
                &messages,
                crate::llm_language::LlmLanguage::PhoenixNative,
                false,
                search_enabled,
                phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
                None,
            );
            let final_request = build_agent_request(
                &messages,
                crate::llm_language::LlmLanguage::PhoenixNative,
                true,
                search_enabled,
                phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
                None,
            );
            assert_eq!(
                planning.tool_availability.declarations(),
                final_request.tool_availability.declarations()
            );
            assert!(final_request.tool_availability.callable_names().is_empty());
            assert_eq!(
                final_request.tool_availability.anthropic_changes().len(),
                planning.tool_availability.declarations().len()
            );
            assert!(final_request
                .tool_availability
                .anthropic_changes()
                .iter()
                .all(|change| change.after_message_id == "chain-qa-results-1"));
        }
    }
}
