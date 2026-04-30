//! Tests for the chain Q&A backend (REQ-CHN-001 / 004 / 005 / 006).

use super::*;
use crate::db::{ChainQaStatus, Database, MessageContent};
use crate::llm::{LlmError, LlmResponse, TokenChunk, Usage};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Build a 3-member linear chain and return the ids in chain order.
async fn build_linear_chain(db: &Database, ids: &[&str]) {
    for id in ids {
        db.create_conversation(id, &format!("slug-{id}"), "/tmp", true, None, None)
            .await
            .unwrap();
    }
    for pair in ids.windows(2) {
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(pair[1])
            .bind(pair[0])
            .execute(db.pool())
            .await
            .unwrap();
    }
}

/// Convenience: shove a continuation summary as the trailing message of a
/// conversation, the way `Effect::persist_continuation_message` does at
/// runtime.
async fn add_continuation_summary(db: &Database, conv_id: &str, summary: &str) {
    let msg_id = format!("msg-cont-{conv_id}");
    db.add_message(
        &msg_id,
        conv_id,
        &MessageContent::continuation(summary),
        None,
        None,
    )
    .await
    .unwrap();
}

async fn add_user_message(db: &Database, conv_id: &str, idx: usize, text: &str) {
    let msg_id = format!("msg-user-{conv_id}-{idx}");
    db.add_message(&msg_id, conv_id, &MessageContent::user(text), None, None)
        .await
        .unwrap();
}

/// Test LLM service: returns a canned text response and counts calls so
/// tests can assert "did the leaf summary call fire?".
#[derive(Debug, Default)]
struct CountingLlm {
    response_text: String,
    calls: AtomicUsize,
}

impl CountingLlm {
    fn new(response_text: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            response_text: response_text.into(),
            calls: AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmService for CountingLlm {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LlmResponse {
            content: vec![ContentBlock::text(self.response_text.clone())],
            end_turn: true,
            usage: Usage::default(),
        })
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        _chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        self.complete(request).await
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait signature requires &str
    fn model_id(&self) -> &str {
        "test-model"
    }
}

// --- compute_chain_snapshot ------------------------------------------------

#[tokio::test]
async fn compute_chain_snapshot_sums_message_counts_across_three_members() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["s-a", "s-b", "s-c"]).await;
    add_user_message(&db, "s-a", 0, "first").await;
    add_user_message(&db, "s-a", 1, "second").await;
    add_continuation_summary(&db, "s-a", "summary of s-a").await;
    add_user_message(&db, "s-b", 0, "first b").await;
    add_continuation_summary(&db, "s-b", "summary of s-b").await;
    add_user_message(&db, "s-c", 0, "first c").await;
    add_user_message(&db, "s-c", 1, "second c").await;
    add_user_message(&db, "s-c", 2, "third c").await;

    let mut members = Vec::new();
    for id in ["s-a", "s-b", "s-c"] {
        members.push(db.get_conversation(id).await.unwrap());
    }
    let snap = compute_chain_snapshot(&members);
    assert_eq!(snap.member_count, 3);
    assert_eq!(snap.total_messages, 3 + 2 + 3);
}

// --- bundle_chain_context --------------------------------------------------

#[tokio::test]
async fn bundle_chain_context_three_member_with_short_leaf() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["m1", "m2", "m3"]).await;
    add_continuation_summary(&db, "m1", "summary M1: built skeleton").await;
    add_continuation_summary(&db, "m2", "summary M2: hooked up auth").await;
    add_user_message(&db, "m3", 0, "leaf hello").await;
    add_user_message(&db, "m3", 1, "leaf world").await;

    let llm = CountingLlm::new("unused");
    let mut members = Vec::new();
    for id in ["m1", "m2", "m3"] {
        members.push(db.get_conversation(id).await.unwrap());
    }
    let bundled = bundle_chain_context(&db, &members, llm.as_ref())
        .await
        .unwrap();

    assert_eq!(bundled.blocks.len(), 3);
    assert_eq!(bundled.blocks[0].kind, MemberBlockKind::ContinuationSummary);
    assert_eq!(bundled.blocks[0].conv_id, "m1");
    assert!(bundled.blocks[0].body.contains("built skeleton"));
    assert_eq!(bundled.blocks[1].kind, MemberBlockKind::ContinuationSummary);
    assert_eq!(bundled.blocks[1].conv_id, "m2");
    assert!(bundled.blocks[1].body.contains("hooked up auth"));
    assert_eq!(bundled.blocks[2].kind, MemberBlockKind::LeafTranscript);
    assert_eq!(bundled.blocks[2].conv_id, "m3");
    assert!(bundled.blocks[2].body.contains("leaf hello"));
    assert_eq!(
        llm.call_count(),
        0,
        "short leaf should not invoke the LLM for summarization",
    );

    let rendered = bundled.render_for_prompt();
    assert!(rendered.contains("[summary:#m1]"));
    assert!(rendered.contains("[summary:#m2]"));
    assert!(rendered.contains("[leaf:#m3]"));
}

#[tokio::test]
async fn bundle_chain_context_long_leaf_summarizes_via_llm() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["L1", "L2"]).await;
    add_continuation_summary(&db, "L1", "first member summary").await;
    // Push the leaf over LEAF_DIRECT_MESSAGE_LIMIT.
    let n = LEAF_DIRECT_MESSAGE_LIMIT + 5;
    for i in 0..n {
        add_user_message(
            &db,
            "L2",
            i,
            "this is a normal user message that takes some bytes",
        )
        .await;
    }

    let llm = CountingLlm::new("(IN-PROCESS LEAF SUMMARY)");
    let mut members = Vec::new();
    for id in ["L1", "L2"] {
        members.push(db.get_conversation(id).await.unwrap());
    }
    let bundled = bundle_chain_context(&db, &members, llm.as_ref())
        .await
        .unwrap();

    assert_eq!(bundled.blocks.len(), 2);
    assert_eq!(bundled.blocks[1].kind, MemberBlockKind::LeafSummary);
    assert_eq!(bundled.blocks[1].body, "(IN-PROCESS LEAF SUMMARY)");
    assert_eq!(
        llm.call_count(),
        1,
        "long leaf should fire one summarization call",
    );
    assert_eq!(bundled.leaf_summary_model.as_deref(), Some("test-model"));
}

#[tokio::test]
async fn bundle_chain_context_marks_missing_continuation_summary() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["mc-a", "mc-b"]).await;
    // mc-a deliberately has NO continuation message — degenerate non-leaf.
    add_user_message(&db, "mc-b", 0, "leaf").await;

    let llm = CountingLlm::new("unused");
    let mut members = Vec::new();
    for id in ["mc-a", "mc-b"] {
        members.push(db.get_conversation(id).await.unwrap());
    }
    let bundled = bundle_chain_context(&db, &members, llm.as_ref())
        .await
        .unwrap();
    assert_eq!(
        bundled.blocks[0].kind,
        MemberBlockKind::ContinuationSummaryMissing,
        "missing summary should surface as a structural tag, not silent drop",
    );
}

// --- ChainQa::submit_question end-to-end (synchronous in Phase 2) ---------

/// Wrap a test `LlmService` in a `ModelRegistry` so `get_mid_tier_model`
/// resolves to it. Uses `ModelRegistry::for_test_with_sonnet`, the
/// test-only constructor on the registry that bypasses gateway plumbing.
fn registry_with_service(service: Arc<dyn LlmService>) -> Arc<ModelRegistry> {
    Arc::new(ModelRegistry::for_test_with_sonnet(service))
}

#[tokio::test]
async fn submit_question_persists_and_completes() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["chq-a", "chq-b", "chq-c"]).await;
    add_continuation_summary(&db, "chq-a", "A summary").await;
    add_continuation_summary(&db, "chq-b", "B summary").await;
    add_user_message(&db, "chq-c", 0, "leaf line").await;

    let llm = CountingLlm::new("THE ANSWER");
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let qa_id = qa
        .submit_question("chq-a", "what happened in this chain?")
        .await
        .unwrap();

    let history = qa.list_history("chq-a").await.unwrap();
    assert_eq!(history.len(), 1);
    let row = &history[0];
    assert_eq!(row.id, qa_id);
    assert_eq!(row.status, ChainQaStatus::Completed);
    assert_eq!(row.question, "what happened in this chain?");
    assert_eq!(row.answer.as_deref(), Some("THE ANSWER"));
    assert_eq!(row.snapshot_member_count, 3);
    assert!(row.completed_at.is_some());
    assert_eq!(row.model, "claude-sonnet-4-6");
    assert_eq!(
        llm.call_count(),
        1,
        "short leaf → no summary call → exactly one answer call",
    );
}

#[tokio::test]
async fn submit_question_rejects_single_member_root() {
    let db = Database::open_in_memory().await.unwrap();
    db.create_conversation("solo-root", "slug-solo", "/tmp", true, None, None)
        .await
        .unwrap();

    let llm = CountingLlm::new("unused");
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let err = qa
        .submit_question("solo-root", "anything")
        .await
        .unwrap_err();
    matches!(err, ChainQaError::NotAChainRoot(_));
}

#[tokio::test]
async fn submit_question_rejects_non_root_member() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["nrr-root", "nrr-mid", "nrr-leaf"]).await;
    add_continuation_summary(&db, "nrr-root", "rs").await;
    add_continuation_summary(&db, "nrr-mid", "ms").await;

    let llm = CountingLlm::new("unused");
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let err = qa.submit_question("nrr-mid", "anything").await.unwrap_err();
    matches!(err, ChainQaError::NotAChainRoot(_));
}
