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
        .submit_question_blocking("chq-a", "what happened in this chain?")
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

// --- Streaming integration (Phase 3) --------------------------------------

use crate::chain_runtime::ChainSseEvent;

/// Streaming test LLM: emits a fixed sequence of token deltas via the
/// streaming channel, then returns an assembled response identical to the
/// concatenated deltas.
#[derive(Debug)]
struct StreamingLlm {
    deltas: Vec<String>,
}

impl StreamingLlm {
    fn new(deltas: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            deltas: deltas.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    fn assembled(&self) -> String {
        self.deltas.concat()
    }
}

#[async_trait]
impl LlmService for StreamingLlm {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Ok(LlmResponse {
            content: vec![ContentBlock::text(self.assembled())],
            end_turn: true,
            usage: Usage::default(),
        })
    }

    async fn complete_streaming(
        &self,
        _request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        for delta in &self.deltas {
            let _ = chunk_tx.send(TokenChunk::Text(delta.clone()));
            // Yield so the forwarder task gets a chance to drain the
            // channel before the next chunk arrives — keeps test ordering
            // deterministic without depending on broadcast capacity.
            tokio::task::yield_now().await;
        }
        Ok(LlmResponse {
            content: vec![ContentBlock::text(self.assembled())],
            end_turn: true,
            usage: Usage::default(),
        })
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &str {
        "test-model"
    }
}

/// Failing streaming LLM: emits one chunk before returning an error.
#[derive(Debug)]
struct FailingStreamingLlm;

#[async_trait]
impl LlmService for FailingStreamingLlm {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::auth("boom"))
    }

    async fn complete_streaming(
        &self,
        _request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let _ = chunk_tx.send(TokenChunk::Text("partial-".to_string()));
        tokio::task::yield_now().await;
        Err(LlmError::auth("simulated stream failure"))
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &str {
        "test-model"
    }
}

/// Drain `n` events from the broadcast receiver. Treats `Lagged` as a hard
/// failure so a misbehaving test fixture doesn't silently mask a missing
/// event.
async fn drain_n(rx: &mut broadcast::Receiver<ChainSseEvent>, n: usize) -> Vec<ChainSseEvent> {
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        let recv = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for chain event");
        match recv {
            Ok(ev) => events.push(ev),
            Err(broadcast::error::RecvError::Closed) => panic!("broadcaster closed early"),
            Err(broadcast::error::RecvError::Lagged(_)) => panic!("subscriber lagged"),
        }
    }
    events
}

#[tokio::test]
async fn submit_question_streams_tokens_and_persists_completed_row() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["st-a", "st-b"]).await;
    add_continuation_summary(&db, "st-a", "summary A").await;
    add_user_message(&db, "st-b", 0, "leaf").await;

    let llm = StreamingLlm::new(&["Hel", "lo, ", "world!"]);
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let runtime = qa.runtime_registry().get_or_create("st-a").await;
    let (mut rx, _guard) = runtime.subscribe();

    let qa_id = qa.submit_question_blocking("st-a", "what?").await.unwrap();

    // Three Token + one Completed = four events.
    let events = drain_n(&mut rx, 4).await;

    // First three are Token in order, all carrying our qa_id.
    let mut deltas: Vec<String> = Vec::new();
    for ev in &events[..3] {
        match ev {
            ChainSseEvent::Token { chain_qa_id, delta } => {
                assert_eq!(chain_qa_id, &qa_id);
                deltas.push(delta.clone());
            }
            other => panic!("expected Token, got {other:?}"),
        }
    }
    assert_eq!(deltas, vec!["Hel", "lo, ", "world!"]);

    // Fourth is Completed with the assembled answer.
    match &events[3] {
        ChainSseEvent::Completed {
            chain_qa_id,
            full_answer,
        } => {
            assert_eq!(chain_qa_id, &qa_id);
            assert_eq!(full_answer, "Hello, world!");
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    // Persisted row reflects status=Completed with the assembled answer.
    let history = qa.list_history("st-a").await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, qa_id);
    assert_eq!(history[0].status, ChainQaStatus::Completed);
    assert_eq!(history[0].answer.as_deref(), Some("Hello, world!"));
}

#[tokio::test]
async fn submit_question_streams_failure_event_and_persists_partial() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["sf-a", "sf-b"]).await;
    add_continuation_summary(&db, "sf-a", "summary A").await;
    add_user_message(&db, "sf-b", 0, "leaf").await;

    let llm: Arc<FailingStreamingLlm> = Arc::new(FailingStreamingLlm);
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let runtime = qa.runtime_registry().get_or_create("sf-a").await;
    let (mut rx, _guard) = runtime.subscribe();

    let qa_id = qa.submit_question_blocking("sf-a", "what?").await.unwrap();

    // Token + Failed.
    let events = drain_n(&mut rx, 2).await;
    match &events[0] {
        ChainSseEvent::Token { chain_qa_id, delta } => {
            assert_eq!(chain_qa_id, &qa_id);
            assert_eq!(delta, "partial-");
        }
        other => panic!("expected Token, got {other:?}"),
    }
    match &events[1] {
        ChainSseEvent::Failed {
            chain_qa_id,
            error: _,
            partial_answer,
        } => {
            assert_eq!(chain_qa_id, &qa_id);
            assert_eq!(partial_answer.as_deref(), Some("partial-"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    let history = qa.list_history("sf-a").await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, ChainQaStatus::Failed);
    assert_eq!(history[0].answer.as_deref(), Some("partial-"));
}

#[tokio::test]
async fn submit_question_returns_qa_id_before_stream_completes() {
    // Submission shape: submit_question returns ChainQaId immediately after
    // INSERT in_flight. The persisted row exists synchronously even though
    // the streaming model invocation happens in a spawned task.
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["sy-a", "sy-b"]).await;
    add_continuation_summary(&db, "sy-a", "summary").await;
    add_user_message(&db, "sy-b", 0, "leaf").await;

    let llm = StreamingLlm::new(&["x"]);
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    let qa_id = qa.submit_question("sy-a", "?").await.unwrap();

    // The row exists in the DB (status may be in_flight or completed
    // depending on timing — the contract is "exists before submit returns").
    let history = qa.list_history("sy-a").await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, qa_id);
}

#[tokio::test]
async fn tab_close_mid_stream_does_not_orphan_invocation() {
    // Subscriber drops mid-stream; the chain runtime stays alive (in-flight
    // count is still 1), the model invocation completes, and the row is
    // updated to Completed. A subscriber that connects late reads the
    // canonical answer from `list_chain_qa` even though it missed the
    // token events.
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["tc-a", "tc-b"]).await;
    add_continuation_summary(&db, "tc-a", "summary").await;
    add_user_message(&db, "tc-b", 0, "leaf").await;

    let llm = StreamingLlm::new(&["one ", "two ", "three"]);
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    // Subscriber connects, then drops before the stream starts.
    let runtime = qa.runtime_registry().get_or_create("tc-a").await;
    {
        let (_rx, _guard) = runtime.subscribe();
        // _rx and _guard go out of scope here, dropping the subscription
        // before the model call runs.
    }
    assert_eq!(runtime.subscriber_count(), 0);

    let qa_id = qa.submit_question_blocking("tc-a", "?").await.unwrap();

    // Persisted row is the canonical state for any late reader.
    let history = qa.list_history("tc-a").await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, qa_id);
    assert_eq!(history[0].status, ChainQaStatus::Completed);
    assert_eq!(history[0].answer.as_deref(), Some("one two three"));
}

#[tokio::test]
async fn chain_runtime_dropped_from_registry_after_qa_completes_with_no_subscribers() {
    let db = Database::open_in_memory().await.unwrap();
    build_linear_chain(&db, &["dr-a", "dr-b"]).await;
    add_continuation_summary(&db, "dr-a", "summary").await;
    add_user_message(&db, "dr-b", 0, "leaf").await;

    let llm = StreamingLlm::new(&["x"]);
    let registry = registry_with_service(llm.clone() as Arc<dyn LlmService>);
    let qa = ChainQa::new(db.clone(), registry);

    qa.submit_question_blocking("dr-a", "?").await.unwrap();

    // After the (synchronous) blocking submit, the in-flight guard has
    // been dropped and there are no subscribers — the registry should
    // have released the runtime.
    assert!(
        !qa.runtime_registry().contains("dr-a").await,
        "registry should drop idle runtimes after Q&A finalize",
    );
}
