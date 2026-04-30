//! Chain-scoped runtime registry for Phoenix Chains v1 (REQ-CHN-004).
//!
//! Phase 3 of Phoenix Chains: introduces a per-chain broadcaster keyed by
//! `root_conv_id`, analogous to [`crate::runtime::ConversationHandle`]'s
//! conversation-scoped broadcaster. Chain broadcasters carry token-streaming
//! events for one or more concurrent Q&A invocations on the same chain;
//! subscribers demultiplex events using each event's `chain_qa_id` field
//! (REQ-CHN-006 — questions are answered against the chain content, not as
//! a thread, so siblings don't render each other's tokens).
//!
//! ### Lifecycle
//!
//! Per `specs/chains/design.md` "Chain broadcaster lifecycle":
//!
//! - Created lazily on the first Q&A submission for a chain
//!   ([`ChainRuntimeRegistry::get_or_create`]).
//! - Subscribers count up at [`ChainRuntime::subscribe`] and down at the
//!   returned [`ChainSubscriberGuard`]'s drop.
//! - In-flight Q&A invocations count up at [`ChainRuntime::begin_qa`] and
//!   down at the returned [`ChainQaInFlightGuard`]'s drop.
//! - Teardown (registry removes the entry) happens when **both** counts
//!   reach zero — see [`ChainRuntimeRegistry::release_if_idle`]. In-flight
//!   pins the runtime alive past zero subscribers so a tab close mid-stream
//!   does not orphan the model invocation; the row's `answer` column in
//!   `chain_qa` is canonical for any subscriber that connects after the
//!   stream ends.
//!
//! ### Why a separate broadcaster from `SseBroadcaster`
//!
//! Conversation broadcasters carry a per-conversation monotonic
//! `sequence_id` (task 02675) consumed by the client's `applyIfNewer` dedup
//! guard. Chain Q&A streams are demultiplexed by `chain_qa_id` and have no
//! reconnect-replay obligation (the persisted `chain_qa` row is the
//! canonical fallback for late readers). Reusing `SseBroadcaster` would
//! conflate two unrelated total-orderings; a fresh broadcaster sharpens the
//! semantic boundary.

#![allow(dead_code)] // Phase 3: registry surface; Phase 4 wires API handlers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

/// Capacity of the per-chain broadcast channel. Sized for ~80s of streaming
/// at ~50 chunks/second across a couple of concurrent Q&As. Mirrors
/// [`crate::runtime::SSE_BROADCAST_CAPACITY`] — same trade-off (overflow ⇒
/// subscriber sees `Lagged`, reconnects, reads canonical state from the DB).
pub const CHAIN_SSE_BROADCAST_CAPACITY: usize = 4096;

/// One token chunk on a streaming chain Q&A.
///
/// Distinct from [`crate::runtime::SseEvent::Token`] because a chain
/// broadcaster does not share the conversation's `sequence_id` counter; the
/// only ordering required across chain events is per-`chain_qa_id` arrival
/// order, which the underlying broadcast channel already provides.
#[derive(Debug, Clone)]
pub enum ChainSseEvent {
    /// A streaming token chunk for an in-flight Q&A.
    Token { chain_qa_id: String, delta: String },
    /// Stream completed cleanly. `full_answer` is the assembled text already
    /// persisted into `chain_qa.answer` by the time this event fires.
    Completed {
        chain_qa_id: String,
        full_answer: String,
    },
    /// Stream ended in error before producing a full answer. `partial_answer`
    /// is whatever was assembled before the failure (may be empty).
    Failed {
        chain_qa_id: String,
        error: String,
        partial_answer: Option<String>,
    },
}

impl ChainSseEvent {
    /// `chain_qa_id` discriminator the subscriber filters on.
    pub fn chain_qa_id(&self) -> &str {
        match self {
            Self::Token { chain_qa_id, .. }
            | Self::Completed { chain_qa_id, .. }
            | Self::Failed { chain_qa_id, .. } => chain_qa_id,
        }
    }
}

/// Per-chain runtime: owns a broadcast channel and two reference counters
/// (subscribers + in-flight Q&As) that together govern teardown.
pub struct ChainRuntime {
    root_conv_id: String,
    tx: broadcast::Sender<ChainSseEvent>,
    subscriber_count: Arc<AtomicUsize>,
    in_flight_count: Arc<AtomicUsize>,
}

impl ChainRuntime {
    fn new(root_conv_id: String) -> Self {
        let (tx, _rx) = broadcast::channel(CHAIN_SSE_BROADCAST_CAPACITY);
        Self {
            root_conv_id,
            tx,
            subscriber_count: Arc::new(AtomicUsize::new(0)),
            in_flight_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Chain identity (root conversation id).
    pub fn root_conv_id(&self) -> &str {
        &self.root_conv_id
    }

    /// Send an event to all current subscribers. Returns the number of
    /// receivers that observed the send (0 when no subscribers); errors are
    /// the same "no active receivers" condition `tokio::broadcast` reports.
    /// Callers in this module ignore the count and rely on the persisted
    /// `chain_qa` row for any post-hoc reader.
    pub fn publish(&self, event: ChainSseEvent) {
        // `send` returns Err only when there are zero receivers; the in-flight
        // path drives the stream regardless of subscriber count, so this is
        // not a meaningful failure mode here.
        let _ = self.tx.send(event);
    }

    /// Subscribe to chain events. The returned guard decrements the
    /// subscriber count when dropped; the caller holds onto it for the
    /// lifetime of the SSE stream and must keep it in scope alongside the
    /// `Receiver` so disconnects properly drop the count.
    pub fn subscribe(
        self: &Arc<Self>,
    ) -> (broadcast::Receiver<ChainSseEvent>, ChainSubscriberGuard) {
        let rx = self.tx.subscribe();
        self.subscriber_count.fetch_add(1, Ordering::AcqRel);
        let guard = ChainSubscriberGuard {
            count: Arc::clone(&self.subscriber_count),
        };
        (rx, guard)
    }

    /// Mark the start of an in-flight Q&A invocation. The returned guard
    /// decrements when dropped; callers should hold it for the entire
    /// streaming window and through the persistence finalize.
    pub fn begin_qa(self: &Arc<Self>) -> ChainQaInFlightGuard {
        self.in_flight_count.fetch_add(1, Ordering::AcqRel);
        ChainQaInFlightGuard {
            count: Arc::clone(&self.in_flight_count),
        }
    }

    /// Current subscriber count. Test-only / diagnostics.
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count.load(Ordering::Acquire)
    }

    /// Current in-flight Q&A count. Test-only / diagnostics.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight_count.load(Ordering::Acquire)
    }

    /// Whether this runtime is idle (no subscribers, no in-flight Q&As).
    pub fn is_idle(&self) -> bool {
        self.subscriber_count() == 0 && self.in_flight_count() == 0
    }
}

/// Drop-scoped subscriber count decrementer. Held by SSE stream handlers.
#[must_use = "ChainSubscriberGuard must be held for the lifetime of the SSE subscription"]
pub struct ChainSubscriberGuard {
    count: Arc<AtomicUsize>,
}

impl Drop for ChainSubscriberGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Drop-scoped in-flight Q&A count decrementer. Held by the streaming task
/// for the entire model-invocation window.
#[must_use = "ChainQaInFlightGuard must be held for the lifetime of the streaming Q&A"]
pub struct ChainQaInFlightGuard {
    count: Arc<AtomicUsize>,
}

impl Drop for ChainQaInFlightGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Process-global registry of [`ChainRuntime`]s keyed by `root_conv_id`.
///
/// Cloneable handle around a shared map. Production code holds a single
/// instance (typically on `AppState`); tests may create their own.
#[derive(Clone, Default)]
pub struct ChainRuntimeRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<ChainRuntime>>>>,
}

impl ChainRuntimeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up an existing runtime for `root_conv_id` or create one.
    /// Always returns a runtime; callers decide via [`ChainRuntime::begin_qa`]
    /// whether to pin it alive.
    pub async fn get_or_create(&self, root_conv_id: &str) -> Arc<ChainRuntime> {
        let mut map = self.inner.lock().await;
        if let Some(existing) = map.get(root_conv_id) {
            return Arc::clone(existing);
        }
        let runtime = Arc::new(ChainRuntime::new(root_conv_id.to_string()));
        map.insert(root_conv_id.to_string(), Arc::clone(&runtime));
        runtime
    }

    /// Look up an existing runtime without creating one. Returns `None` when
    /// no runtime is registered for the chain.
    pub async fn get(&self, root_conv_id: &str) -> Option<Arc<ChainRuntime>> {
        self.inner.lock().await.get(root_conv_id).cloned()
    }

    /// Drop the runtime for `root_conv_id` if it is currently idle (no
    /// subscribers, no in-flight Q&As). No-op if the entry is missing or
    /// non-idle. Returns `true` when an entry was actually removed.
    ///
    /// Called after a subscriber's stream ends and after each Q&A finalizes.
    /// The check holds the registry lock so we can't race a fresh subscriber
    /// who is about to bump the count: a `subscribe()` call goes through
    /// `get_or_create`, which takes the same lock first.
    pub async fn release_if_idle(&self, root_conv_id: &str) -> bool {
        let mut map = self.inner.lock().await;
        let Some(rt) = map.get(root_conv_id) else {
            return false;
        };
        if rt.is_idle() {
            map.remove(root_conv_id);
            tracing::debug!(root_conv_id = %root_conv_id, "chain runtime idle, dropped from registry");
            true
        } else {
            false
        }
    }

    /// Number of registered runtimes (test-only diagnostic).
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Whether `root_conv_id` has a registered runtime (test-only).
    pub async fn contains(&self, root_conv_id: &str) -> bool {
        self.inner.lock().await.contains_key(root_conv_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_creates_runtime_lazily_on_first_reference() {
        let reg = ChainRuntimeRegistry::new();
        assert_eq!(reg.len().await, 0);

        let _rt = reg.get_or_create("root-A").await;
        assert!(reg.contains("root-A").await);
        assert_eq!(reg.len().await, 1);

        // Same key returns the same Arc.
        let rt2 = reg.get_or_create("root-A").await;
        let rt1 = reg.get("root-A").await.unwrap();
        assert!(Arc::ptr_eq(&rt1, &rt2));
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn registry_release_if_idle_drops_idle_runtime() {
        let reg = ChainRuntimeRegistry::new();
        let _rt = reg.get_or_create("root-A").await;
        assert!(reg.contains("root-A").await);

        let removed = reg.release_if_idle("root-A").await;
        assert!(removed);
        assert!(!reg.contains("root-A").await);
        assert_eq!(reg.len().await, 0);

        // Idempotent: calling again on a missing key is a no-op.
        let removed = reg.release_if_idle("root-A").await;
        assert!(!removed);
    }

    #[tokio::test]
    async fn release_if_idle_keeps_runtime_with_active_subscriber() {
        let reg = ChainRuntimeRegistry::new();
        let rt = reg.get_or_create("root-S").await;
        let (_rx, _guard) = rt.subscribe();

        let removed = reg.release_if_idle("root-S").await;
        assert!(
            !removed,
            "runtime with active subscriber must not be dropped"
        );
        assert!(reg.contains("root-S").await);
    }

    #[tokio::test]
    async fn release_if_idle_keeps_runtime_with_in_flight_qa() {
        let reg = ChainRuntimeRegistry::new();
        let rt = reg.get_or_create("root-F").await;
        let _qa_guard = rt.begin_qa();
        assert_eq!(rt.in_flight_count(), 1);

        let removed = reg.release_if_idle("root-F").await;
        assert!(
            !removed,
            "in-flight Q&A must pin the runtime alive past zero subscribers",
        );
        assert!(reg.contains("root-F").await);
    }

    #[tokio::test]
    async fn in_flight_pins_runtime_after_subscriber_drops() {
        let reg = ChainRuntimeRegistry::new();
        let rt = reg.get_or_create("root-P").await;

        let qa_guard = rt.begin_qa();
        let (_rx, sub_guard) = rt.subscribe();
        assert_eq!(rt.subscriber_count(), 1);
        assert_eq!(rt.in_flight_count(), 1);

        // Subscriber drops while the Q&A is still streaming.
        drop(sub_guard);
        assert_eq!(rt.subscriber_count(), 0);
        let removed = reg.release_if_idle("root-P").await;
        assert!(
            !removed,
            "in-flight stream must keep broadcaster alive past zero subscribers",
        );

        // Q&A reaches terminal — both counts now zero.
        drop(qa_guard);
        assert!(rt.is_idle());
        let removed = reg.release_if_idle("root-P").await;
        assert!(removed);
        assert!(!reg.contains("root-P").await);
    }

    #[tokio::test]
    async fn subscriber_guard_decrements_on_drop() {
        let reg = ChainRuntimeRegistry::new();
        let rt = reg.get_or_create("root-G").await;

        let (_rx1, g1) = rt.subscribe();
        let (_rx2, g2) = rt.subscribe();
        assert_eq!(rt.subscriber_count(), 2);

        drop(g1);
        assert_eq!(rt.subscriber_count(), 1);
        drop(g2);
        assert_eq!(rt.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn multi_subscriber_demux_via_chain_qa_id() {
        let reg = ChainRuntimeRegistry::new();
        let rt = reg.get_or_create("root-M").await;

        let (mut rx1, _g1) = rt.subscribe();
        let (mut rx2, _g2) = rt.subscribe();

        // Two concurrent Q&As publish onto the same chain broadcaster.
        rt.publish(ChainSseEvent::Token {
            chain_qa_id: "qa-A".to_string(),
            delta: "hello ".to_string(),
        });
        rt.publish(ChainSseEvent::Token {
            chain_qa_id: "qa-B".to_string(),
            delta: "world ".to_string(),
        });
        rt.publish(ChainSseEvent::Completed {
            chain_qa_id: "qa-A".to_string(),
            full_answer: "hello there".to_string(),
        });
        rt.publish(ChainSseEvent::Failed {
            chain_qa_id: "qa-B".to_string(),
            error: "boom".to_string(),
            partial_answer: Some("world".to_string()),
        });

        // Both subscribers see all four events in order, each tagged with its
        // own chain_qa_id so a UI subscriber can filter to its own question.
        for rx in [&mut rx1, &mut rx2] {
            let mut seen: Vec<String> = Vec::new();
            for _ in 0..4 {
                let ev = rx.recv().await.unwrap();
                seen.push(ev.chain_qa_id().to_string());
            }
            assert_eq!(seen, vec!["qa-A", "qa-B", "qa-A", "qa-B"]);
        }
    }

    #[tokio::test]
    async fn chain_sse_event_chain_qa_id_extracts_consistently() {
        let cases = [
            ChainSseEvent::Token {
                chain_qa_id: "tk".to_string(),
                delta: "x".to_string(),
            },
            ChainSseEvent::Completed {
                chain_qa_id: "cp".to_string(),
                full_answer: "y".to_string(),
            },
            ChainSseEvent::Failed {
                chain_qa_id: "fl".to_string(),
                error: "z".to_string(),
                partial_answer: None,
            },
        ];
        let ids: Vec<&str> = cases.iter().map(ChainSseEvent::chain_qa_id).collect();
        assert_eq!(ids, vec!["tk", "cp", "fl"]);
    }
}
