//! Chain Q&A backend (REQ-CHN-001, REQ-CHN-004, REQ-CHN-005, REQ-CHN-006).
//!
//! Phase 2 of Phoenix Chains v1: bundles per-member context, invokes a
//! mid-tier model with the user's question, and persists the resulting Q&A
//! row through its lifecycle (`in_flight` → `completed` | `failed`).
//!
//! Phase 3 will move the answer-generation invocation onto a streaming path
//! and wrap it in a chain-scoped SSE broadcaster. The bundling and
//! persistence helpers in this module are shaped so that swap is a
//! reuse-and-rewrap, not a rewrite — see [`ChainQa::run_answer_invocation`].

// Phase 2 ships the backend in isolation; Phase 3 wires it into runtime
// streaming and Phase 4 surfaces it via API handlers. Until then, the public
// surface is exercised only by the in-module tests, which clippy reads as
// "never used in non-test code". Same idiom as the chain DB methods in
// Phase 1 (`#[allow(dead_code)] // Callers added in Phase 2`).
#![allow(dead_code)]

use crate::db::{
    ChainQaRow, Conversation, Database, DbError, Message, MessageContent, MessageType, NewChainQa,
};
use crate::llm::{
    ContentBlock, LlmError, LlmMessage, LlmRequest, LlmService, MessageRole, ModelRegistry,
    SystemContent,
};
use chrono::Utc;
use std::sync::Arc;

/// Maximum leaf message count for direct (un-summarized) inclusion (REQ-CHN-006).
///
/// Pinned alongside [`LEAF_DIRECT_TOKEN_BUDGET`]: if either threshold is
/// exceeded, the leaf is summarized in-process before invocation. Pinning
/// these as constants instead of per-call inputs prevents identical
/// questions on the same chain from getting different bundling decisions.
pub const LEAF_DIRECT_MESSAGE_LIMIT: usize = 20;

/// Maximum approximate-token budget for a directly-included leaf transcript.
/// Token approximation uses `text.len() / 4` (REQ-CHN-006 spec); when the
/// leaf transcript exceeds this, it is summarized via the mid-tier model
/// in-process and discarded after the request.
pub const LEAF_DIRECT_TOKEN_BUDGET: usize = 4000;

/// System prompt for the chain Q&A answer invocation (REQ-CHN-001).
const ANSWER_SYSTEM_PROMPT: &str = "You are answering a question about a Phoenix continuation chain — \
a sequence of conversations that were continued one into the next as the original conversation \
exhausted its context. The user's question is below the bundled context.

Each chain member is delimited by a structural tag (e.g. [main:#abc123] or [leaf-summary:#def456]). \
Answer ONLY from the bundled chain content. If the context does not support a confident answer, \
say so explicitly and indicate what would be needed to answer. Do not speculate beyond the \
provided content.";

/// System prompt for the in-process leaf-summary pre-step.
const LEAF_SUMMARY_SYSTEM_PROMPT: &str =
    "Summarize the work done in the conversation transcript below. \
Focus on what was attempted, what was decided, what was completed, and any open questions. \
Aim for a concise summary (a few short paragraphs) that another LLM could use to answer \
recall questions about this conversation. Do not include greetings, sign-offs, or commentary \
about the summary itself — just the summary.";

/// Maximum tokens cap for the in-process leaf summary.
const LEAF_SUMMARY_MAX_TOKENS: u32 = 1024;

/// Maximum tokens cap for the answer invocation. Sized to a typical recall
/// answer; the model can stop earlier via `end_turn`.
const ANSWER_MAX_TOKENS: u32 = 2048;

/// Result of bundling a chain into model-ready context blocks.
#[derive(Debug, Clone)]
pub struct BundledContext {
    /// One block per chain member, in chain order (root → leaf).
    pub blocks: Vec<MemberContextBlock>,
    /// `model_id` used for any leaf-summary pre-step (None if the leaf was
    /// taken directly). Threaded through for diagnostics; not persisted.
    pub leaf_summary_model: Option<String>,
}

impl BundledContext {
    /// Render the bundled context as a single string suitable for use as
    /// the user-message body of the answer invocation.
    pub fn render_for_prompt(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            let tag = block.kind.tag(&block.conv_id);
            out.push('[');
            out.push_str(&tag);
            out.push_str("]\n");
            out.push_str(&block.body);
            if !block.body.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }
}

/// One context block contributed by a single chain member.
#[derive(Debug, Clone)]
pub struct MemberContextBlock {
    pub conv_id: String,
    pub kind: MemberBlockKind,
    pub body: String,
}

/// Distinguishes the four ways a member can contribute to the bundle.
///
/// The kind is the structural label rendered into the prompt — making
/// "this came from the persisted continuation summary" vs. "this is the
/// in-process leaf summary because the leaf was too big" visible to both
/// the model and to humans reading transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberBlockKind {
    /// Non-leaf member — body is its trailing `MessageType::Continuation`
    /// summary, persisted by `Effect::persist_continuation_message` during
    /// the AwaitingContinuation→ContextExhausted transition.
    ContinuationSummary,
    /// Non-leaf member that has no trailing Continuation message in the DB
    /// (a degenerate state — the chain edge exists but the summary message
    /// was never persisted). Surfaced as a logged-debug capability gap and
    /// a structural tag rather than silently dropped.
    ContinuationSummaryMissing,
    /// Leaf member — body is the raw transcript (≤ thresholds in
    /// [`LEAF_DIRECT_MESSAGE_LIMIT`] / [`LEAF_DIRECT_TOKEN_BUDGET`]).
    LeafTranscript,
    /// Leaf member — body is an in-process LLM summary (transcript exceeded
    /// the direct budget). Held in memory only; not persisted (see design.md).
    LeafSummary,
}

impl MemberBlockKind {
    fn tag(self, conv_id: &str) -> String {
        let prefix = match self {
            Self::ContinuationSummary => "summary",
            Self::ContinuationSummaryMissing => "summary-missing",
            Self::LeafTranscript => "leaf",
            Self::LeafSummary => "leaf-summary",
        };
        format!("{prefix}:#{conv_id}")
    }
}

/// Snapshot of chain shape captured at question-submission time
/// (REQ-CHN-005). Two integers replace what would otherwise be a JSON
/// snapshot of the full member graph; the UI compares these against
/// current chain state to decide whether to show a "snapshot stale" tag.
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

/// Identifier returned to the caller of [`ChainQa::submit_question`] —
/// doubles as the SSE-stream demux key in Phase 3.
pub type ChainQaId = String;

/// Chain Q&A entry point. Owns no per-chain state in Phase 2; it is a
/// thin façade over the database, model registry, and bundling helpers.
/// Phase 3 will introduce a per-chain runtime for streaming, but the
/// public submission shape stays the same.
pub struct ChainQa {
    db: Database,
    llm_registry: Arc<ModelRegistry>,
}

impl ChainQa {
    pub fn new(db: Database, llm_registry: Arc<ModelRegistry>) -> Self {
        Self { db, llm_registry }
    }

    /// Submit a question on the chain rooted at `root_id`. Phase 2 runs the
    /// answer invocation synchronously (Phase 3 swaps in streaming).
    ///
    /// Returns the `chain_qa_id` of the persisted row — Phase 3 will use it
    /// to demux SSE token events between concurrent subscribers.
    ///
    /// The flow is split into helpers that Phase 3 will reuse:
    /// 1. [`Self::prepare_invocation`] — validate, load members, snapshot,
    ///    bundle context, INSERT the `in_flight` row.
    /// 2. [`Self::run_answer_invocation`] — make the model call. Phase 3
    ///    rewrites this to call `complete_streaming` and dispatch tokens
    ///    onto the chain broadcaster, but the surrounding flow is unchanged.
    /// 3. [`Self::finalize`] — UPDATE the row to `completed` or `failed`.
    pub async fn submit_question(
        &self,
        root_id: &str,
        question: &str,
    ) -> Result<ChainQaId, ChainQaError> {
        let prep = self.prepare_invocation(root_id, question).await?;
        let qa_id = prep.row_id.clone();
        let invocation_result = self.run_answer_invocation(&prep).await;
        self.finalize(&qa_id, invocation_result).await?;
        Ok(qa_id)
    }

    /// Phase 1 of the submission flow.
    ///
    /// Validates the chain, snapshots its shape, bundles its context, and
    /// INSERTs the row in `in_flight` — all *before* the answer invocation
    /// fires, so the question is durable even if the model call panics
    /// mid-flight (REQ-CHN-005: question text is preserved across failure
    /// modes).
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

        let bundled = bundle_chain_context(&self.db, &members, service.as_ref()).await?;

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
            bundled,
            service,
            model_id,
        })
    }

    /// Phase 2 of the submission flow — the model invocation.
    ///
    /// Phase 3 replaces this body with a streaming path; the surrounding
    /// `prepare_invocation` and `finalize` calls do not change. Errors here
    /// flow through `finalize` and become `status = failed`.
    async fn run_answer_invocation(
        &self,
        prep: &PreparedInvocation,
    ) -> Result<String, ChainQaError> {
        let request = build_answer_request(&prep.bundled, &prep.question);
        let response = prep.service.complete(&request).await?;
        Ok(response.text())
    }

    /// Phase 3 of the submission flow — terminal status transition.
    async fn finalize(
        &self,
        qa_id: &str,
        result: Result<String, ChainQaError>,
    ) -> Result<(), ChainQaError> {
        match result {
            Ok(answer) => {
                self.db
                    .complete_chain_qa(qa_id, &answer, Utc::now())
                    .await?;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(qa_id = %qa_id, error = %e, "chain Q&A invocation failed");
                self.db.fail_chain_qa(qa_id, None).await?;
                Err(e)
            }
        }
    }

    /// Read-side: fetch persisted Q&A history for a chain (REQ-CHN-005).
    pub async fn list_history(&self, root_id: &str) -> Result<Vec<ChainQaRow>, ChainQaError> {
        Ok(self.db.list_chain_qa(root_id).await?)
    }
}

/// Per-submission state passed from `prepare_invocation` to
/// `run_answer_invocation` and `finalize`. Phase 3 will extend this with a
/// broadcaster handle but the existing fields stay the same.
struct PreparedInvocation {
    row_id: ChainQaId,
    question: String,
    bundled: BundledContext,
    service: Arc<dyn LlmService>,
    #[allow(dead_code)] // Persisted into chain_qa.model via insert_chain_qa
    model_id: String,
}

/// Build the answer-time `LlmRequest` from a bundled context and a question.
fn build_answer_request(bundled: &BundledContext, question: &str) -> LlmRequest {
    let prompt = format!(
        "{context}\n---\nQuestion: {question}\n",
        context = bundled.render_for_prompt(),
        question = question,
    );
    LlmRequest {
        system: vec![SystemContent::new(ANSWER_SYSTEM_PROMPT)],
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(prompt)],
        }],
        tools: vec![],
        max_tokens: Some(ANSWER_MAX_TOKENS),
    }
}

/// Bundle a chain's pre-loaded members into model-ready context blocks
/// (REQ-CHN-001).
///
/// Caller passes `members` already loaded as `Conversation`s (so
/// `message_count` is populated from the SELECT). The leaf member's body
/// is either its raw transcript (when ≤ thresholds) or an in-process
/// summary generated via `service`.
pub async fn bundle_chain_context(
    db: &Database,
    members: &[Conversation],
    service: &dyn LlmService,
) -> Result<BundledContext, ChainQaError> {
    if members.is_empty() {
        return Ok(BundledContext {
            blocks: vec![],
            leaf_summary_model: None,
        });
    }

    let mut blocks: Vec<MemberContextBlock> = Vec::with_capacity(members.len());
    let leaf_idx = members.len() - 1;
    let mut leaf_summary_model: Option<String> = None;

    for (i, conv) in members.iter().enumerate() {
        if i == leaf_idx {
            let transcript = db.get_messages(&conv.id).await?;
            let direct_text = render_leaf_transcript(&transcript);
            let approx_tokens = approx_token_count(&direct_text);

            if transcript.len() <= LEAF_DIRECT_MESSAGE_LIMIT
                && approx_tokens <= LEAF_DIRECT_TOKEN_BUDGET
            {
                blocks.push(MemberContextBlock {
                    conv_id: conv.id.clone(),
                    kind: MemberBlockKind::LeafTranscript,
                    body: direct_text,
                });
            } else {
                tracing::debug!(
                    conv_id = %conv.id,
                    msg_count = transcript.len(),
                    approx_tokens,
                    "Chain leaf exceeds direct budget; summarizing in-process",
                );
                let summary = summarize_leaf_in_process(service, &direct_text).await?;
                leaf_summary_model = Some(service.model_id().to_string());
                blocks.push(MemberContextBlock {
                    conv_id: conv.id.clone(),
                    kind: MemberBlockKind::LeafSummary,
                    body: summary,
                });
            }
        } else {
            // Non-leaf: pull the trailing Continuation message.
            let messages = db.get_messages(&conv.id).await?;
            if let Some(text) = trailing_continuation_summary(&messages) {
                blocks.push(MemberContextBlock {
                    conv_id: conv.id.clone(),
                    kind: MemberBlockKind::ContinuationSummary,
                    body: text,
                });
            } else {
                tracing::debug!(
                    conv_id = %conv.id,
                    "Non-leaf chain member missing trailing Continuation message; \
                     emitting summary-missing tag",
                );
                blocks.push(MemberContextBlock {
                    conv_id: conv.id.clone(),
                    kind: MemberBlockKind::ContinuationSummaryMissing,
                    body: String::from("(no continuation summary persisted for this member)"),
                });
            }
        }
    }

    Ok(BundledContext {
        blocks,
        leaf_summary_model,
    })
}

/// Approximate token count via `len / 4` (REQ-CHN-006 spec; exact
/// tokenization is out of scope for v1).
fn approx_token_count(text: &str) -> usize {
    text.len() / 4
}

/// Render a leaf transcript as a human-readable plain-text block.
///
/// Tool calls and tool results are folded into compact one-line markers so
/// the leaf budget heuristic isn't dominated by JSON. Continuation messages
/// inside a leaf transcript would be unusual but are passed through verbatim.
fn render_leaf_transcript(messages: &[Message]) -> String {
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
            MessageContent::Tool(c) => format!("(tool result: {} chars)", c.content.len()),
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

/// Find the **trailing** `MessageType::Continuation` message and extract
/// its summary. Returns None when the conversation has no Continuation
/// message at all (degenerate non-leaf state).
fn trailing_continuation_summary(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| match &m.content {
        MessageContent::Continuation(c) => Some(c.summary.clone()),
        _ => None,
    })
}

/// Generate an in-process leaf summary via the same mid-tier model. The
/// result is held in memory only; not persisted (see design.md "Leaf
/// summarization is in-process, not persisted").
async fn summarize_leaf_in_process(
    service: &dyn LlmService,
    transcript_text: &str,
) -> Result<String, ChainQaError> {
    let request = LlmRequest {
        system: vec![SystemContent::new(LEAF_SUMMARY_SYSTEM_PROMPT)],
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(transcript_text.to_string())],
        }],
        tools: vec![],
        max_tokens: Some(LEAF_SUMMARY_MAX_TOKENS),
    };
    let response = service.complete(&request).await?;
    Ok(response.text())
}

#[cfg(test)]
mod tests;
