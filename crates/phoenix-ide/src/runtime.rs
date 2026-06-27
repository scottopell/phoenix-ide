#![allow(clippy::wildcard_enum_match_arm)]
//! Runtime for executing conversations
//!
//! REQ-BED-007: State Persistence
//! REQ-BED-010: Fixed Working Directory
//! REQ-BED-011: Real-time Event Streaming

//! REQ-BED-012: Context Window Tracking
//! REQ-BED-008: Sub-Agent Spawning
//! REQ-BED-009: Sub-Agent Isolation

pub mod deny_gate;
pub(crate) mod executor;
pub(crate) mod fork_resolve;
mod recovery;
pub mod traits;
pub mod usage_limit_sweep;
pub mod user_facing_error;

#[cfg(test)]
pub mod testing;

pub use executor::ConversationRuntime;
pub use traits::*;

use crate::platform::PlatformCapability;
use crate::state_machine::state::{ModeKind, SubAgentMode, SubAgentOutcome, SubAgentSpec};
use crate::tools::browser::session::BrowserSessionLifecycleEvent;
use crate::tools::{
    BashHandleRegistry, BashLifecycleEvent, BrowserSessionManager, ExploreToolPolicy,
    TmuxLifecycleEvent, TmuxRegistry, ToolRegistry,
};
use phoenix_core::work_scope::WorkScope;

/// Type alias for production runtime with concrete implementations
pub type ProductionRuntime =
    ConversationRuntime<DatabaseStorage, RegistryLlmClient, ToolRegistryExecutor>;

use crate::db::{ConvMode, Database};
use crate::state_machine::{ConvContext, ConvState, Event};
use crate::system_prompt::ModeContext;
use chrono::{DateTime, Utc};
use phoenix_llm::ModelRegistry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot, watch, RwLock};

/// Request to spawn a sub-agent
#[derive(Debug)]
pub struct SubAgentSpawnRequest {
    pub spec: SubAgentSpec,
    pub parent_conversation_id: String,
    pub parent_event_tx: mpsc::Sender<Event>,
}

/// Request to cancel sub-agents
#[derive(Debug)]
pub struct SubAgentCancelRequest {
    pub ids: Vec<String>,
    #[allow(dead_code)] // Used for logging/debugging
    pub parent_conversation_id: String,
    pub parent_event_tx: mpsc::Sender<Event>,
}

/// Why a runtime was evicted. Passed to `evict_runtime` so the next
/// `get_or_create` can describe the real cause in the auto-continue recovery
/// message instead of always blaming a server restart (task 02710).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    /// The conversation's model was changed; the runtime is recreated so the
    /// new model takes effect.
    ModelUpgrade,
}

#[derive(Debug)]
pub struct TaskApprovalHandoffRequest {
    pub parent_conversation_id: String,
    pub approval: TaskApprovalHandoffData,
    pub response_tx: oneshot::Sender<Result<TaskApprovalHandoffResponse, String>>,
}

pub use phoenix_core::task_handoff::TaskApprovalHandoffData;

#[derive(Debug, Clone)]
pub struct TaskApprovalHandoffResponse {
    pub successor_conv_id: String,
}

/// Manager for all conversation runtimes
pub struct RuntimeManager {
    db: Database,
    llm_registry: Arc<ModelRegistry>,
    platform: PlatformCapability,
    browser_sessions: Arc<BrowserSessionManager>,
    /// Per-process bash handle registry. Shared by every conversation's
    /// `ToolContext`; each `WorkScope` gets its own `WorkScopeHandles`
    /// table inside, so a continuation chain on one worktree shares one
    /// table (REQ-BASH-014, REQ-BASH-WS-001).
    bash_handles: Arc<BashHandleRegistry>,
    /// Per-process tmux server registry. Shared by every conversation's
    /// `ToolContext`; each conversation gets its own `Arc<RwLock<TmuxServer>>`
    /// inside, keyed by `conversation_id`. The `which("tmux")` result is
    /// cached on construction (REQ-TMUX-001 / REQ-TMUX-013).
    tmux_registry: Arc<TmuxRegistry>,
    mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
    /// Active PTY terminal sessions — threaded into `ToolContext` for `read_terminal`.
    pub terminals: crate::terminal::ActiveTerminals,
    runtimes: RwLock<HashMap<String, ConversationHandle>>,
    /// Broadcasters from evicted runtimes, waiting to be inherited by a
    /// replacement runtime created by the next `get_or_create` call.
    ///
    /// `evict_runtime` deposits the old broadcaster here instead of dropping
    /// it, so existing SSE clients remain subscribed to the same channel and
    /// continue receiving events once the new runtime starts broadcasting on
    /// it. Without inheritance the clients would sit on a dead channel until
    /// the axum keep-alive ping eventually expired or the user refreshed.
    evicted_broadcasters: RwLock<HashMap<String, SseBroadcaster>>,
    /// Why each pending-eviction runtime was evicted, keyed by conversation
    /// id. Deposited by `evict_runtime` alongside the broadcaster and consumed
    /// by the next `get_or_create` so the auto-continue recovery message says
    /// "model was upgraded" rather than "server restart" (task 02710). A plain
    /// in-process server restart clears this map, so absence = server restart.
    /// A set, not a map: model upgrade is currently the only eviction cause;
    /// add a `HashMap<String, EvictionReason>` if a second cause appears.
    ///
    /// Lifetime is bounded and self-limiting: consumed by the next
    /// `get_or_create`, dropped by `take_evicted_broadcaster` on hard-delete,
    /// and the whole in-memory set is gone on process restart. Worst case is
    /// a handful of small `String`s for conversations evicted-for-upgrade but
    /// never re-accessed within one process lifetime — not worth a TTL.
    evicted_model_upgrades: RwLock<HashSet<String>>,
    /// Channel for sub-agent spawn requests
    spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
    spawn_rx: RwLock<Option<mpsc::Receiver<SubAgentSpawnRequest>>>,
    /// Channel for sub-agent cancel requests
    cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    cancel_rx: RwLock<Option<mpsc::Receiver<SubAgentCancelRequest>>>,
    handoff_tx: mpsc::Sender<TaskApprovalHandoffRequest>,
    handoff_rx: RwLock<Option<mpsc::Receiver<TaskApprovalHandoffRequest>>>,
    /// Channel to the single serialized fork-resolution consumer. Every fork
    /// proposal resolution (approve / request-changes) and cleanup (dismiss /
    /// retire-on-terminal / hard-delete) is a [`fork_resolve::ForkCommand`]
    /// processed one at a time by that consumer, so mutual exclusion between a
    /// resolve and a cleanup is structural rather than lock-based.
    fork_cmd_tx: mpsc::Sender<fork_resolve::ForkCommand>,
    fork_cmd_rx: RwLock<Option<mpsc::Receiver<fork_resolve::ForkCommand>>>,
    /// Credential helper for recovery settlement (REQ-BED-030).
    credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
    /// Receiver for browser session lifecycle edges. Taken once by
    /// [`RuntimeManager::start_browser_lifecycle_bridge`] which spawns a
    /// task that resolves `conversation_id` to its `SseBroadcaster` and
    /// broadcasts [`SseEvent::BrowserSessionState`].
    browser_lifecycle_rx: RwLock<Option<mpsc::UnboundedReceiver<BrowserSessionLifecycleEvent>>>,
    /// Receiver for bash state-transition signals (spawn / terminal / kill).
    /// Taken once by [`RuntimeManager::start_work_scope_bridge`], which
    /// assembles the affected scope's inventory and broadcasts
    /// [`SseEvent::WorkScopeUpdate`] (REQ-WSUI-007).
    bash_lifecycle_rx: RwLock<Option<mpsc::UnboundedReceiver<BashLifecycleEvent>>>,
    /// Receiver for tmux state-transition signals (entry created / status
    /// change / cascade removal). Taken once by
    /// [`RuntimeManager::start_work_scope_bridge`], which assembles the
    /// affected scope's inventory and broadcasts [`SseEvent::WorkScopeUpdate`].
    /// Opening a conversation's terminal panel materializes a tmux entry via
    /// `ensure_live`; this is the edge that pushes it to the work-scope panel
    /// (REQ-WSUI-007).
    tmux_lifecycle_rx: RwLock<Option<mpsc::UnboundedReceiver<TmuxLifecycleEvent>>>,
    /// Sender the browser lifecycle bridge forwards a `WorkScope` into after
    /// it broadcasts a `BrowserSessionState` edge, so the work-scope bridge
    /// also emits a `WorkScopeUpdate` for that scope (REQ-WSUI-007: a browser
    /// liveness edge is a work-scope change). Reuses the browser bridge's
    /// scope resolution rather than introducing a second mechanism.
    work_scope_browser_tx: mpsc::UnboundedSender<WorkScope>,
    /// Matching receiver, taken once by `start_work_scope_bridge`.
    work_scope_browser_rx: RwLock<Option<mpsc::UnboundedReceiver<WorkScope>>>,
}

/// Handle to interact with a running conversation
pub struct ConversationHandle {
    pub event_tx: mpsc::Sender<Event>,
    /// SSE broadcaster. Owns the per-conversation monotonic `sequence_id` counter
    /// that every emitted [`SseEvent`] must consume (task 02675). Callers never
    /// hand-craft a `sequence_id` — they either go through [`SseBroadcaster::send_seq`]
    /// (which allocates the next id from the counter) or [`SseBroadcaster::send_message`]
    /// (which passes through the DB-allocated message id and advances the counter past
    /// it). This makes the "every SSE event carries a monotonic `sequence_id`" contract
    /// structurally enforceable rather than a matter of caller discipline.
    pub broadcast_tx: SseBroadcaster,
    /// Opaque per-instance identity used by cleanup tasks to guard against
    /// removing a _replacement_ entry that was created after eviction. Each
    /// call to `get_or_create` allocates a fresh `Arc<()>`; all clones of a
    /// handle share the same pointer so `Arc::ptr_eq` is a reliable identity
    /// check even across cheap clones.
    identity: Arc<()>,
    /// Live-state observer. The executor writes to the paired sender on every
    /// state transition; readers call `state_rx.borrow()` to get the current
    /// executor state without acquiring the `runtimes` lock or touching the DB.
    /// Authority rule: if a handle exists, its `state_rx` is authoritative for
    /// transient in-flight state; the DB row is the safe rest-state fallback
    /// when no handle is present (see `effective_conversation_state`).
    pub(crate) state_rx: watch::Receiver<ConvState>,
}

/// Capacity of the per-conversation SSE broadcast channel.
///
/// Sized to cover a realistic worst-case stall of the slowest receiver
/// (a background tab, a sleeping laptop resume, a long GC pause) during
/// active LLM streaming. At ~50 tokens/sec this buys ~80 seconds of headroom.
///
/// When the channel overflows, `BroadcastStreamRecvError::Lagged` fires on
/// the receive side. We handle that in `api::sse::sse_stream` by closing the
/// stream — the client reconnects, `init` replays current state, and no
/// silent gap results. Increasing this value reduces how often that resync
/// dance happens; it does not change correctness.
pub const SSE_BROADCAST_CAPACITY: usize = 4096;

/// Capacity of the per-conversation `ReplayRing` (`sse_wire.allium`
/// `replay_ring_capacity`). Bounds how many ephemeral events between two
/// persisted Messages can be replayed on reconnect. At 50 tokens/sec this
/// covers ~10 seconds of LLM streaming; overflow forces a full resync
/// (the ring clears and the truncated flag is set; subsequent appends are
/// no-ops until the next persisted Message resets the anchor).
pub const REPLAY_RING_CAPACITY: usize = 512;

/// One entry in the per-conversation `ReplayRing`. Carries the original
/// `sequence_id` allocated by the broadcaster when the event was emitted,
/// so the client's `applyIfNewer` guard works identically on replay.
#[derive(Debug, Clone)]
pub struct ReplayRingEntry {
    pub event: SseEvent,
    pub sequence_id: i64,
}

/// Per-conversation buffer of ephemeral SSE events between persisted-Message
/// anchors. Source-of-truth for `sse_wire.allium`'s `ReplayRing` entity. See the
/// spec for invariants:
///   - `entries` has length ≤ `REPLAY_RING_CAPACITY`.
///   - Every entry has `sequence_id > anchor_seq`.
///   - Entries are in strictly increasing `sequence_id` order (preserved by
///     FIFO append + anchor-reset clear; no out-of-order insertion paths
///     exist).
///   - Once `truncated = true`, no further appends accumulate until the
///     next anchor reset.
///
/// Byte-size observability is lazy: see [`ReplayRing::total_bytes`].
/// Tokens are the dominant append path during LLM streaming and we
/// refuse to pay a `serde_json::to_vec` on every one of them just to
/// keep a running counter. Bytes are computed on demand at the
/// truncation log line and through the `replay_ring_bytes()` accessor.
#[derive(Debug)]
struct ReplayRing {
    entries: VecDeque<ReplayRingEntry>,
    /// Sequence id of the most recent persisted Message; everything in
    /// `entries` has seq > this. Starts at 0 for a fresh conversation.
    anchor_seq: i64,
    /// True iff the ring has overflowed since the last anchor reset.
    /// Once true, subsequent appends are no-ops. Cleared by `reset`.
    truncated: bool,
}

impl ReplayRing {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            anchor_seq: 0,
            truncated: false,
        }
    }

    /// Anchor reset on persisted Message broadcast. Clears entries,
    /// advances anchor, clears truncation flag.
    fn reset(&mut self, new_anchor: i64) {
        self.entries.clear();
        self.anchor_seq = new_anchor;
        self.truncated = false;
    }

    /// Append an entry. If accepting it would exceed `REPLAY_RING_CAPACITY`,
    /// transition to truncated state: clear the ring and set
    /// `truncated = true`. Subsequent appends within this anchor window
    /// are no-ops. The next `reset` clears the truncated flag.
    ///
    /// Concurrent-allocation safety: `SseBroadcaster::send_seq` allocates
    /// `next_seq()` outside this ring mutex, then builds the event, then
    /// re-acquires the mutex to append. Two concurrent senders can
    /// therefore interleave such that a higher seq locks the ring first
    /// and a lower seq locks second. Two guards keep the ring consistent:
    ///
    /// 1. Entries with `seq <= anchor_seq` are dropped on append. Covers
    ///    the case where a persisted-Message broadcast slips in between
    ///    a sender's `next_seq` and its ring-append: the anchor advances
    ///    past the late sender's seq, and the late entry is for a
    ///    superseded message anyway.
    /// 2. `snapshot()` sorts entries by `sequence_id`. Out-of-order
    ///    insertion at append time is then corrected at read time, so
    ///    replay delivers events in the order the client's
    ///    `applyIfNewer` expects.
    fn append(&mut self, entry: ReplayRingEntry) {
        if self.truncated {
            return;
        }
        // Concurrent-allocation guard #1: drop seqs that no longer beat
        // the current anchor. A persisted Message broadcast may have
        // raced ahead of this sender, advancing the anchor past its seq.
        if entry.sequence_id <= self.anchor_seq {
            tracing::trace!(
                target: "phoenix_ide::replay_ring",
                seq = entry.sequence_id,
                anchor = self.anchor_seq,
                "ReplayRing append dropped: seq below anchor (concurrent reset raced ahead)"
            );
            return;
        }
        if self.entries.len() >= REPLAY_RING_CAPACITY {
            // One-shot bytes computation at the truncation transition.
            // This is the only place we pay the serialisation cost during
            // an anchor window — ops gets a useful "what was in the ring
            // when it overflowed?" data point without making every token
            // append a hot-path serde call.
            let bytes = self.total_bytes();
            tracing::warn!(
                target: "phoenix_ide::replay_ring",
                cap = REPLAY_RING_CAPACITY,
                anchor = self.anchor_seq,
                bytes,
                "ReplayRing capacity reached; clearing and entering truncated mode (next subscribe will force-resync)"
            );
            self.entries.clear();
            self.truncated = true;
            return;
        }
        tracing::trace!(
            target: "phoenix_ide::replay_ring",
            seq = entry.sequence_id,
            entries = self.entries.len() + 1,
            "ReplayRing append"
        );
        self.entries.push_back(entry);
    }

    /// Snapshot for delivery in `SseEvent::Init`: returns the anchor, the
    /// truncated flag, the highest `sequence_id` covered by the snapshot,
    /// and a clone of the entries (empty if truncated, per Q3 resolution:
    /// force full resync rather than partial replay).
    ///
    /// `highest_seq` is the upper bound the caller uses to set the init
    /// payload's `last_sequence_id`. With non-empty entries it is the seq
    /// of the last entry (after sorting); with empty entries (including
    /// the truncated case) it is the anchor. The caller computes
    /// `init.last_sequence_id = max(db_last_seq, highest_seq)` so the
    /// spec invariant `entry.sequence_id <= snapshot.last_sequence_id`
    /// (`sse_wire.allium` `StreamOpened`) holds even if a sender broadcast
    /// raced the handler between the DB read and the ring snapshot. Using
    /// `highest_seq` (not the broadcaster's current counter) as the floor
    /// also lets any in-flight broadcast — one that allocated a seq via
    /// `next_seq()` but had not yet appended to the ring at snapshot time
    /// — pass the client's `applyIfNewer` guard on its live delivery,
    /// because its seq strictly exceeds `highest_seq`.
    ///
    /// Concurrent-allocation guard #2: entries are sorted by
    /// `sequence_id` here. The append path can receive entries out of seq
    /// order when two senders race between `next_seq()` and the ring
    /// mutex (see [`ReplayRing::append`] for details). The client's
    /// `applyIfNewer` reducer relies on strictly increasing seqs to apply
    /// per-event rules, so the read path establishes the ordering rather
    /// than the write path (which would require holding seq allocation
    /// inside the ring mutex and serialising every broadcast).
    fn snapshot(&self) -> (i64, bool, i64, Vec<SseEvent>) {
        let (highest_seq, events) = if self.truncated {
            (self.anchor_seq, Vec::new())
        } else {
            let mut entries: Vec<&ReplayRingEntry> = self.entries.iter().collect();
            entries.sort_by_key(|e| e.sequence_id);
            let highest = entries.last().map_or(self.anchor_seq, |e| e.sequence_id);
            let events = entries.into_iter().map(|e| e.event.clone()).collect();
            (highest, events)
        };
        (self.anchor_seq, self.truncated, highest_seq, events)
    }

    /// Aggregate serialised JSON byte length across current ring entries.
    /// Lazy / on-demand — iterates and serialises each entry via the
    /// `SseWireEvent` conversion (matching the production wire path).
    /// NOT a hot-path metric: call from the truncation log line or the
    /// `replay_ring_bytes()` ops accessor, not from per-event append.
    fn total_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|e| {
                let wire: crate::api::wire::SseWireEvent = e.event.clone().into();
                serde_json::to_vec(&wire).map(|v| v.len()).unwrap_or(0)
            })
            .sum()
    }
}

/// What to do with the per-conversation `ReplayRing` when broadcasting an
/// event. The variant is determined at the public-API surface (e.g.
/// `send_persisted_message` selects `Anchor`; `send_seq` selects `Append`),
/// not inferred from the `SseEvent` variant — because two different code
/// paths emit `SseEvent::Message` (persisted vs eager) and the ring
/// lifecycle differs between them.
#[derive(Debug, Clone, Copy)]
enum RingOp {
    /// Reset the ring with `seq` as the new anchor. Used by the persisted
    /// Message broadcast path (the DB row is now durable; ephemeral events
    /// below this seq need not be replayed).
    Anchor,
    /// Append the event to the ring. Used for ephemeral events emitted via
    /// `send_seq` and for eager (non-persisted) Message broadcasts.
    Append,
}

/// Per-conversation SSE broadcaster with monotonic `sequence_id` allocation.
///
/// Every [`SseEvent`] emitted for a conversation carries a `sequence_id` drawn
/// from a single per-conversation counter. This broadcaster is the sole
/// gateway: callers cannot construct a [`SseEvent`] and broadcast it without
/// first obtaining a `sequence_id` from here, which means the total-order
/// invariant is enforced by the type rather than by caller discipline.
///
/// Three broadcast paths exist:
///
/// 1. **Ephemeral/derived events** (`Token`, `StateChange`, `MessageUpdated`, …) —
///    allocate a fresh id via [`SseBroadcaster::next_seq`] or use
///    [`SseBroadcaster::send_seq`], which hands the id to a construction
///    closure so the caller cannot forget to insert it. These append to
///    the `ReplayRing` so a reconnect mid-turn can replay them.
///
/// 2. **Persisted `Message` events** already carry a `message.sequence_id`
///    allocated by `add_message` in the DB layer. Use
///    [`SseBroadcaster::send_persisted_message`], which reuses that id,
///    advances the broadcaster's counter past it, and **resets** the
///    `ReplayRing` anchor (the DB row is now durable, so ephemeral events
///    below it are no longer needed for replay).
///
/// 3. **Eager (non-persisted) Message events** are broadcast before their
///    corresponding `persist_checkpoint` runs (see
///    `Effect::BroadcastAssistantMessage`). Use
///    [`SseBroadcaster::send_ephemeral_message`], which appends to the
///    `ReplayRing` like an ephemeral event so reconnecting clients still
///    see the in-flight assistant message. The eventual persisted Message
///    with the same `message_id` will fire path #2, clearing this entry.
#[derive(Clone)]
pub struct SseBroadcaster {
    tx: broadcast::Sender<SseEvent>,
    /// Highest `sequence_id` emitted so far for this conversation.
    /// `next_seq()` returns `fetch_add(1)` + 1 atomically; `observe_seq(s)`
    /// bumps this value up to at least `s` so message-originated ids integrate
    /// into the same total order.
    last_seq: Arc<AtomicI64>,
    /// Per-conversation `ReplayRing` (`sse_wire.allium`). Shared across
    /// clones of this `SseBroadcaster` so every broadcast path mutates
    /// the same buffer. Mutex contention is acceptable: every broadcast
    /// is already serialised through tokio's broadcast channel.
    ring: Arc<Mutex<ReplayRing>>,
}

impl SseBroadcaster {
    /// Build a broadcaster from an existing `broadcast::Sender`.
    ///
    /// `initial_last_seq` is the highest `sequence_id` the client can already
    /// have observed (typically `db.get_last_sequence_id(conversation_id)`).
    /// The next allocated id will be `initial_last_seq + 1`.
    ///
    /// The `ReplayRing` is seeded with `anchor_seq = initial_last_seq` so
    /// the first ephemeral event broadcast has its seq strictly greater
    /// than the anchor.
    pub fn from_sender(tx: broadcast::Sender<SseEvent>, initial_last_seq: i64) -> Self {
        let mut ring = ReplayRing::new();
        ring.anchor_seq = initial_last_seq;
        Self {
            tx,
            last_seq: Arc::new(AtomicI64::new(initial_last_seq)),
            ring: Arc::new(Mutex::new(ring)),
        }
    }

    /// Construct a broadcaster with a fresh broadcast channel.
    /// Convenience for call sites that also need the underlying channel's
    /// capacity configured.
    pub fn new(channel_capacity: usize, initial_last_seq: i64) -> Self {
        let (tx, _rx) = broadcast::channel(channel_capacity);
        Self::from_sender(tx, initial_last_seq)
    }

    /// Atomically allocate the next `sequence_id` and return it.
    pub fn next_seq(&self) -> i64 {
        self.last_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Bump the internal counter so subsequent `next_seq()` values are strictly
    /// greater than `seq`. No-op if the counter is already past `seq`.
    ///
    /// Called for `Message` events whose `sequence_id` is allocated by the DB
    /// layer — we have to fold those into the same total order without
    /// double-allocating.
    pub fn observe_seq(&self, seq: i64) {
        self.last_seq.fetch_max(seq, Ordering::AcqRel);
    }

    /// Highest `sequence_id` emitted so far. Used to seed `SseEvent::Init`'s
    /// `last_sequence_id` so the client's `applyIfNewer` guard starts at the
    /// correct floor.
    #[allow(dead_code)]
    pub fn current_seq(&self) -> i64 {
        self.last_seq.load(Ordering::Acquire)
    }

    /// Subscribe to the SSE broadcast stream.
    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.tx.subscribe()
    }

    /// Number of active receivers on the underlying broadcast channel.
    /// Used for diagnostics — a count of 0 when broadcasting means events
    /// are being sent to a channel with no SSE clients subscribed
    /// (possible indicator of the "spinner-forever" stranding scenario).
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Send an event that has already been stamped with a `sequence_id`,
    /// applying the requested `ReplayRing` operation atomically with the
    /// broadcast.
    ///
    /// Order: the ring mutation happens **before** the channel send. This
    /// guarantees that any concurrent subscriber observing the ring after
    /// the broadcast sees a state consistent with what the live channel
    /// would deliver — never a state where the broadcast happened but the
    /// ring lifecycle hasn't yet (which would let a fresh subscribe miss
    /// an event in the gap).
    ///
    /// Returns `Ok(receiver_count)` on success, `Err(())` when the channel has
    /// no active receivers. The error payload is discarded on purpose —
    /// `broadcast::error::SendError<SseEvent>` is ~320 bytes, which triggers
    /// clippy's `result_large_err` lint, and every call site here only ever
    /// reads `.is_err()`.
    fn send_with_ring(&self, event: SseEvent, seq: i64, op: RingOp) -> Result<usize, ()> {
        match op {
            RingOp::Anchor => {
                self.ring.lock().expect("ReplayRing mutex").reset(seq);
            }
            RingOp::Append => {
                self.ring
                    .lock()
                    .expect("ReplayRing mutex")
                    .append(ReplayRingEntry {
                        event: event.clone(),
                        sequence_id: seq,
                    });
            }
        }
        self.tx.send(event).map_err(|_| ())
    }

    /// Allocate the next `sequence_id`, pass it to `build`, broadcast the
    /// resulting event, and append it to the `ReplayRing` so reconnects
    /// can replay it. The closure's signature forces the caller to place
    /// the id on the event — forgetting is a compile error.
    pub fn send_seq(&self, build: impl FnOnce(i64) -> SseEvent) -> Result<usize, ()> {
        let seq = self.next_seq();
        let event = build(seq);
        self.send_with_ring(event, seq, RingOp::Append)
    }

    /// Broadcast a persisted `Message` event using the DB-allocated
    /// `message.sequence_id`, advance the broadcaster's counter, AND reset
    /// the `ReplayRing` anchor (the DB row is now durable, so ephemeral
    /// events below this seq are no longer needed for replay).
    pub fn send_persisted_message(&self, message: crate::db::Message) -> Result<usize, ()> {
        let seq = message.sequence_id;
        self.observe_seq(seq);
        self.send_with_ring(SseEvent::Message { message }, seq, RingOp::Anchor)
    }

    /// Backward-compatible alias for [`SseBroadcaster::send_persisted_message`].
    /// Existing call sites that broadcast persisted messages can keep using
    /// `send_message`; the eager (non-persisted) path is the new entry point
    /// [`SseBroadcaster::send_ephemeral_message`].
    pub fn send_message(&self, message: crate::db::Message) -> Result<usize, ()> {
        self.send_persisted_message(message)
    }

    /// Broadcast an eager (non-persisted) `Message` event and append it to
    /// the `ReplayRing` WITHOUT resetting the anchor. Used by
    /// `Effect::BroadcastAssistantMessage` to deliver the in-flight assistant
    /// message during a tool round, before `persist_checkpoint` produces the
    /// durable DB row.
    ///
    /// The eventual persisted Message with the same `message_id` will go
    /// through [`SseBroadcaster::send_persisted_message`] when the tool round
    /// checkpoints, which resets the ring (discarding this entry) and emits
    /// a duplicate `sse_message` that the UI dedups via
    /// `SseMessageDedupReplay`.
    pub fn send_ephemeral_message(&self, message: crate::db::Message) -> Result<usize, ()> {
        let seq = message.sequence_id;
        self.observe_seq(seq);
        self.send_with_ring(SseEvent::Message { message }, seq, RingOp::Append)
    }

    /// Atomic snapshot of the `ReplayRing` for delivery in `SseEvent::Init`.
    ///
    /// Returns `(anchor, truncated, highest_seq, events)`. See
    /// [`ReplayRing::snapshot`] for `highest_seq` semantics — it is the
    /// floor the caller uses to set `Init.last_sequence_id`.
    /// Returns `(pending_anchor_sequence_id, pending_truncated,
    /// pending_events)`. When truncated, `pending_events` is empty — forcing
    /// a full DB-only resync on the client side per the Q3 resolution in
    /// `sse_wire.allium`.
    pub fn snapshot_pending(&self) -> (i64, bool, i64, Vec<SseEvent>) {
        self.ring.lock().expect("ReplayRing mutex").snapshot()
    }

    /// Aggregate serialised byte size of `ReplayRing` entries, computed
    /// on demand. Observability-only; exposed for callers that want to
    /// surface the metric (e.g. a future ops dashboard / Prometheus gauge).
    ///
    /// Cost: O(entries) serialisations. Do not call on the hot path —
    /// scrape periodically from a gauge collector or read at truncation
    /// time only.
    #[allow(dead_code)]
    pub fn replay_ring_bytes(&self) -> usize {
        self.ring.lock().expect("ReplayRing mutex").total_bytes()
    }
}

/// Typed update for conversation metadata pushed mid-session.
/// Each field is `Option` — only populated fields are serialized to the client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConversationMetadataUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conv_mode_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CachedPrSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub display_state: phoenix_core::domain::pr_display_state::PrDisplayState,
    pub base: String,
    pub head: String,
}

/// A conversation enriched with derived display fields for the API layer.
///
/// Produces the same JSON shape as the old `conversation_to_json()` `Value`:
/// all `Conversation` fields at the top level (via `#[serde(flatten)]`) plus
/// the extra display fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnrichedConversation {
    #[serde(flatten)]
    pub inner: crate::db::Conversation,
    pub conv_mode_label: String,
    pub branch_name: Option<String>,
    pub worktree_path: Option<String>,
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
    /// The server-user's `$SHELL` (REQ-TERM-002), used by the frontend to
    /// tailor the OSC 133 enablement snippet (REQ-TERM-017).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// The server-user's `$HOME` (REQ-SEED-*), used by the frontend to spawn
    /// seeded conversations scoped to the user's home directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<String>,
    /// Slug of the seed parent conversation, resolved for the UI breadcrumb
    /// (REQ-SEED-003). `None` if `inner.seed_parent_id` is `None` or the
    /// parent has been deleted — the UI renders unlinked text in the latter
    /// case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_parent_slug: Option<String>,
    /// Slug of the sub-agent's parent conversation, resolved for the UI
    /// breadcrumb. `None` if `inner.parent_conversation_id` is `None` (the
    /// conversation is not a sub-agent) or the parent has been deleted — the
    /// UI renders unlinked text in the latter case. Mirrors `seed_parent_slug`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_conversation_slug: Option<String>,
    /// Whether the conversation currently has a live browser session in
    /// `BrowserSessionManager`. Read directly from the manager's `HashMap`
    /// at hydration — single source of truth, no parallel bool. The
    /// running session is updated via `SseEvent::BrowserSessionState`
    /// after init.
    pub browser_session_active: bool,
    /// True when the in-app terminal for this conversation attaches to a
    /// per-conversation tmux session (the default whenever `tmux` is on
    /// PATH; see `TmuxRegistry::binary_available`). The UI uses this to
    /// label terminal-selection snippets with `tmux pane main:1.0` (first
    /// window 1 because `base-index 1` in `tools/tmux/server.conf`, first
    /// pane 0), hinting to the LLM that the existing `tmux` tool can pull
    /// the full pane on follow-up. False when the PTY runs a direct
    /// `$SHELL`.
    pub terminal_uses_tmux: bool,
    /// `WorkScope::stable_key()` for this conversation's resolved `WorkScope`.
    /// The frontend uses it to build the work-scope inventory URL
    /// (`GET /api/work-scope/:scope_key/inventory`). Resolved from the
    /// conversation id + worktree path, the same inputs the
    /// `browser_session_active` lookup uses.
    pub work_scope_key: String,
    /// Compact DB-backed PR association for this conversation's work scope.
    /// Rich status still comes from the PR status endpoint; this snapshot is
    /// only enough to render a stable PR link before that refresh completes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_pr: Option<CachedPrSummary>,
}

/// Events sent to SSE clients.
///
/// Every variant carries a `sequence_id` drawn from the conversation's single
/// monotonic counter (task 02675). The client's `applyIfNewer` guard relies on
/// this total order to dedup reconnect replays. Allocation is the
/// responsibility of [`SseBroadcaster`] — do not hand-craft `sequence_id`
/// values at call sites.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum SseEvent {
    Init {
        /// Snapshot's own place in the total order. On init this equals
        /// `last_sequence_id` — the snapshot is itself an event.
        sequence_id: i64,
        conversation: Box<EnrichedConversation>,
        messages: Vec<crate::db::Message>,
        agent_working: bool,
        /// Presentation mode for UI display (`idle`/`working`/`needs_action`/`error`/`done`)
        presentation_mode: String,
        /// Highest `sequence_id` ever emitted for this conversation — what the
        /// client seeds `atom.lastSequenceId` with so subsequent
        /// `applyIfNewer` checks start at the right floor.
        last_sequence_id: i64,
        /// Current context window usage in tokens
        context_window_size: u64,
        /// Human-readable project name derived from the repo root directory name.
        project_name: Option<String>,
        /// `sequence_id` of the most recent persisted Message at subscribe
        /// time. Every entry in `pending_events` has `sequence_id` strictly
        /// greater than this. Equals `initial_last_seq` for a fresh
        /// conversation with no broadcasts since the last persisted Message
        /// (or no persisted Messages yet). See `sse_wire.allium`
        /// `InitSnapshot.pending_anchor_sequence_id`.
        pending_anchor_sequence_id: i64,
        /// `ReplayRing` contents at subscribe time: ephemeral SSE events
        /// broadcast since the last persisted Message anchor that have not
        /// yet been folded into a durable DB row. The client reducer replays
        /// these through its per-event rules after applying the DB snapshot
        /// so a reconnect mid-turn resumes the in-flight view (streaming
        /// tokens, current tool phase, eager assistant message before its
        /// tool round completes). Empty when `pending_truncated = true`
        /// (force full resync; see `sse_wire.allium` Q3 resolution).
        pending_events: Vec<SseEvent>,
        /// True iff the `ReplayRing` overflowed since the last anchor and
        /// `pending_events` is therefore empty by construction. The client
        /// renders the DB snapshot and waits for the next live event to
        /// repopulate the in-flight UI.
        pending_truncated: bool,
    },
    /// A newly-persisted message joins the conversation. Uses `message.sequence_id`
    /// as its envelope `sequence_id` — no separate field needed because
    /// `message.sequence_id` is already the DB-allocated id and, thanks to
    /// [`SseBroadcaster::send_message`], folds into the broadcaster's counter.
    Message {
        message: crate::db::Message,
    },
    /// An existing message's mutable fields changed. Carries only the delta —
    /// `message_id` is the target; `sequence_id` is the envelope id used by
    /// the client reducer for dedup (task 02675). The message's persistent
    /// `sequence_id` is immutable and not repeated here.
    MessageUpdated {
        sequence_id: i64,
        message_id: String,
        display_data: Option<serde_json::Value>,
        content: Option<crate::db::MessageContent>,
        /// Typed tool-execution duration in milliseconds, emitted alongside
        /// the tool-result `Message` event so the client can display elapsed
        /// time without an opaque `display_data` parse. `None` when emitting
        /// from non-tool-result paths (e.g. sub-agent summary).
        duration_ms: Option<u64>,
    },
    StateChange {
        sequence_id: i64,
        /// Full typed conversation state
        state: ConvState,
        /// Presentation mode for UI display (`idle`/`working`/`needs_action`/`error`/`done`)
        presentation_mode: String,
        /// Server clock at which the conversation entered this state — the
        /// same `Conversation.state_updated_at` value the runtime bumps on
        /// every state transition. Specs: `specs/working-phase-visibility/`
        /// REQ-WPV-001.
        state_updated_at: DateTime<Utc>,
    },
    /// Ephemeral streaming token. Not persisted, but still carries a
    /// `sequence_id` from the same counter so reconnects don't strand tokens
    /// behind a per-connection closure counter (task 02675 fixes the
    /// `lastSequence` leapfrog stall).
    Token {
        sequence_id: i64,
        text: String,
        request_id: String,
    },
    /// Emitted exactly once per LLM request immediately before the first
    /// `Token` event for that request, so the client can transition the
    /// `StateBar`'s base reason from `awaiting LLM response Ns` (pre-first-byte)
    /// to `streaming` (post-first-byte) per REQ-WPV-007. NOT emitted
    /// when an LLM request completes with zero tokens (errors, early
    /// termination). Spec: `specs/working-phase-visibility/`.
    LlmFirstByte {
        sequence_id: i64,
        /// Matches the `request_id` carried on `Token` events for the same
        /// LLM request, so the client can correlate the first-byte
        /// transition with the right `StateBar` phase.
        request_id: String,
    },
    /// Retry-context marker emitted from the executor's
    /// `Effect::ScheduleRetry` handler immediately before the spawned
    /// backoff sleep. Carries everything the `StateBar`'s retry suffix
    /// needs — `(retry K/N <reason>)` per
    /// specs/working-phase-visibility/ REQ-WPV-003 — so the user can
    /// distinguish "rate-limit retry storm" from a wedged server during
    /// the otherwise-silent backoff window. Replays via the ephemeral
    /// SSE ring so mid-backoff reconnects reconstruct the suffix.
    /// Specs: `specs/llm-retry-visibility/`, REQ-LRV-001 .. REQ-LRV-003.
    LlmAttempt {
        sequence_id: i64,
        /// 1-indexed attempt this retry is scheduled FOR; matches the
        /// `state.attempt` carried on the next `StateChange`.
        attempt: u32,
        /// Retry-budget ceiling (`MAX_RETRY_ATTEMPTS` from the state
        /// machine — currently 3). Carried per-event so a future
        /// per-provider policy is wire-compatible without a spec change.
        max_attempts: u32,
        /// Classified reason — one of `rate_limit`, `server_error`,
        /// `network` (the retryable subset of `LlmErrorKind`).
        reason: phoenix_llm::LlmAttemptReason,
        /// `delay` from `Effect::ScheduleRetry`, in milliseconds.
        /// Informational at v1 (the client doesn't count down); the
        /// field exists so a future "backing off Ns" sub-display can be
        /// added without a wire change.
        backing_off_ms: u64,
        /// Upstream quota window reset timestamp when the rate-limit
        /// response included one (`None` for `server_error` / `network`
        /// retries and for `rate_limit` retries whose 429 lacked a
        /// `resets_at`). RFC3339 string on the wire.
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    AgentDone {
        sequence_id: i64,
    },
    /// Emitted once when a conversation's `is_terminal()` first becomes true.
    /// Consumed by the terminal subsystem to tear down any active PTY session.
    ConversationBecameTerminal {
        sequence_id: i64,
    },
    /// Pushed when conversation metadata changes mid-session (e.g., cwd/mode after approval).
    /// Typed struct instead of `Value` — the executor knows exactly which fields changed.
    ConversationUpdate {
        sequence_id: i64,
        update: ConversationMetadataUpdate,
    },
    /// User-facing error for the SSE `error` channel. Carries a typed
    /// payload (task 24682) so internal `Debug`-format strings cannot
    /// accidentally leak — every construction goes through
    /// `runtime::user_facing_error`.
    Error {
        sequence_id: i64,
        error: user_facing_error::UserFacingError,
    },
    /// REQ-BED-032 step 6: emitted exactly once after a hard-delete cascade
    /// completes. UI consumers (sidebar, navigation) use it to refresh
    /// views. The `conversation_id` field is redundant with the broadcaster
    /// scope today (the per-conversation channel implies the id) but is
    /// carried explicitly so a future user-scope broadcaster can
    /// disambiguate without changing the wire shape.
    ConversationHardDeleted {
        sequence_id: i64,
        conversation_id: String,
    },
    /// Browser session liveness changed for this conversation. Emitted on
    /// the create edge (`active = true`, fired only on actual `HashMap`
    /// insertion in `BrowserSessionManager::get_session`) and the destroy
    /// edge (`active = false`, fired only when a session was actually
    /// removed — kill, idle cleanup). The UI uses this to drive the live
    /// "browser session running" indicator without inferring it from the
    /// presence of `browser_*` tool calls in message history.
    BrowserSessionState {
        sequence_id: i64,
        active: bool,
    },
    /// A steering message was accepted and queued for delivery when the
    /// conversation next reaches `Idle`. The UI uses this to show the message
    /// with a "Queued" indicator instead of "Sending...".
    SteerMessageQueued {
        sequence_id: i64,
        message_id: String,
        /// Zero-based position in the steering queue.
        queue_position: usize,
    },
    /// Codex-bridge quota snapshot. Emitted once per turn on success (parsed
    /// from `x-codex-*` response headers in `openai.rs::complete_streaming`)
    /// and once on a terminal 429 (replayed from `UsageLimitReached.details`
    /// in `runtime/executor.rs`). Ephemeral — not persisted server-side. The
    /// client stores the latest snapshot in a module-level store and clears
    /// it on codex sign-out / account switch (SSE disconnects alone do not
    /// invalidate the snapshot — the account is unchanged across reconnects).
    RateLimitSnapshot {
        sequence_id: i64,
        snapshot: phoenix_llm::QuotaDetails,
    },
    /// A work-affine resource in this conversation's `WorkScope` changed
    /// state (bash handle spawned / went terminal / was killed, or a browser
    /// session crossed a liveness edge). Carries the full refreshed
    /// `WorkScopeInventory` snapshot — not a delta (REQ-WSUI-007) — assembled
    /// from the live registries by the work-scope bridge. The UI panel
    /// updates from this without re-polling the pull endpoint.
    WorkScopeUpdate {
        sequence_id: i64,
        inventory: phoenix_core::domain::work_scope_inventory::WorkScopeInventory,
    },
}

/// Pick the tool registry for a sub-agent runtime on (re-)creation from
/// its persisted `conv_mode`.
///
/// Explore sub-agents are always persisted as `ConvMode::Explore`
/// (see `handle_spawn_request`); Work sub-agents inherit the parent's
/// `conv_mode`, which is one of Direct, Work, or Branch (never Explore --
/// an Explore parent cannot spawn a Work sub-agent, guarded at spawn
/// time). So `Explore` variant means an Explore sub-agent, anything
/// else means a Work sub-agent. See subagents.allium
/// `SubAgentRegistryOnResume`.
fn sub_agent_registry_for_conv_mode(
    conv_mode: &ConvMode,
    policy: ExploreToolPolicy,
) -> ToolRegistry {
    match conv_mode {
        ConvMode::Explore { .. } => ToolRegistry::for_subagent_explore(policy),
        ConvMode::Direct | ConvMode::Work { .. } | ConvMode::Branch { .. } => {
            ToolRegistry::for_subagent_work()
        }
    }
}

impl RuntimeManager {
    pub fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        platform: PlatformCapability,
        mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
        credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
    ) -> Self {
        let (spawn_tx, spawn_rx) = mpsc::channel(32);
        let (cancel_tx, cancel_rx) = mpsc::channel(32);
        let (handoff_tx, handoff_rx) = mpsc::channel(32);
        let (fork_cmd_tx, fork_cmd_rx) = mpsc::channel(32);
        // Browser session lifecycle channel. Unbounded because the volume is
        // O(user-clicks-on-browser-tools) — a tightly bounded channel could
        // drop edges and desync the UI's "session live" indicator. The
        // matching `Receiver` is consumed by `start_browser_lifecycle_bridge`.
        let (browser_lifecycle_tx, browser_lifecycle_rx) = mpsc::unbounded_channel();
        // Bash state-transition channel (spawn / terminal / kill). Unbounded
        // for the same reason as the browser channel: dropping an edge would
        // desync the work-scope panel's view of the scope's resources.
        let (bash_lifecycle_tx, bash_lifecycle_rx) = mpsc::unbounded_channel();
        // Tmux state-transition channel (entry created / status change /
        // cascade removal). Unbounded for the same reason as the bash
        // channel: dropping an edge would leave the work-scope panel showing
        // a stale tmux row (notably: a terminal-only conversation whose
        // tmux entry never reaches the panel).
        let (tmux_lifecycle_tx, tmux_lifecycle_rx) = mpsc::unbounded_channel();
        // Browser-edge → work-scope forward channel. The browser lifecycle
        // bridge sends the affected scope here after broadcasting its own
        // edge, so the work-scope bridge re-broadcasts a `WorkScopeUpdate`.
        let (work_scope_browser_tx, work_scope_browser_rx) = mpsc::unbounded_channel();
        Self {
            db,
            llm_registry,
            platform,
            browser_sessions: BrowserSessionManager::with_lifecycle_sink(Some(
                browser_lifecycle_tx,
            )),
            bash_handles: Arc::new(BashHandleRegistry::with_lifecycle_sink(Some(
                bash_lifecycle_tx,
            ))),
            tmux_registry: Arc::new(TmuxRegistry::with_lifecycle_sink(Some(tmux_lifecycle_tx))),
            mcp_manager,
            terminals: crate::terminal::ActiveTerminals::new(),
            runtimes: RwLock::new(HashMap::new()),
            evicted_broadcasters: RwLock::new(HashMap::new()),
            evicted_model_upgrades: RwLock::new(HashSet::new()),
            spawn_tx,
            spawn_rx: RwLock::new(Some(spawn_rx)),
            cancel_tx,
            cancel_rx: RwLock::new(Some(cancel_rx)),
            handoff_tx,
            handoff_rx: RwLock::new(Some(handoff_rx)),
            fork_cmd_tx,
            fork_cmd_rx: RwLock::new(Some(fork_cmd_rx)),
            credential_helper,
            browser_lifecycle_rx: RwLock::new(Some(browser_lifecycle_rx)),
            bash_lifecycle_rx: RwLock::new(Some(bash_lifecycle_rx)),
            tmux_lifecycle_rx: RwLock::new(Some(tmux_lifecycle_rx)),
            work_scope_browser_tx,
            work_scope_browser_rx: RwLock::new(Some(work_scope_browser_rx)),
        }
    }

    /// Get the detected platform capability
    #[allow(dead_code)]
    pub fn platform(&self) -> PlatformCapability {
        self.platform.clone()
    }

    /// Get the browser session manager
    pub fn browser_sessions(&self) -> &Arc<BrowserSessionManager> {
        &self.browser_sessions
    }

    /// Get the bash handle registry (REQ-BASH-007 shutdown kill-tree,
    /// REQ-BASH-006 hard-delete cascade).
    pub fn bash_handles(&self) -> &Arc<BashHandleRegistry> {
        &self.bash_handles
    }

    /// Get the tmux server registry (REQ-TMUX-007 hard-delete cascade,
    /// terminal attach path).
    pub fn tmux_registry(&self) -> &Arc<TmuxRegistry> {
        &self.tmux_registry
    }

    /// Get the spawn channel sender (cloned for each runtime)
    #[allow(dead_code)] // Used internally by get_or_create
    fn spawn_tx(&self) -> mpsc::Sender<SubAgentSpawnRequest> {
        self.spawn_tx.clone()
    }

    /// Get the cancel channel sender (cloned for each runtime)
    #[allow(dead_code)] // Used internally by get_or_create
    fn cancel_tx(&self) -> mpsc::Sender<SubAgentCancelRequest> {
        self.cancel_tx.clone()
    }

    /// Start the bridge task that converts `BrowserSessionManager` lifecycle
    /// edges into per-conversation SSE broadcasts. Must be called once after
    /// `RuntimeManager::new`. If the receiver was already taken (double
    /// call) this is a no-op.
    pub async fn start_browser_lifecycle_bridge(self: &Arc<Self>) {
        // Gate browser idle reaping on scope liveness: the cleanup task must
        // not force-close Chrome while the scope still owns a non-terminal
        // conversation. A `Weak` keeps the closure from holding the runtime
        // alive; if the runtime is gone the scope is by definition not live.
        let weak = Arc::downgrade(self);
        self.browser_sessions()
            .set_scope_liveness_hook(Arc::new(move |scope: WorkScope| {
                let weak = weak.clone();
                Box::pin(async move {
                    match weak.upgrade() {
                        Some(manager) => match manager.scope_has_live_conversation(&scope).await {
                            Ok(live) => live,
                            // The idle reaper fails closed: an unreadable DB
                            // (transient lock contention during cleanup) must
                            // not reap a still-live session, so treat the scope
                            // as live and try again on the next idle sweep.
                            Err(e) => {
                                tracing::warn!(
                                    work_scope = %scope,
                                    error = %e,
                                    "scope liveness query failed; preserving scope to avoid reaping a live browser session"
                                );
                                true
                            }
                        },
                        None => false,
                    }
                }) as futures::future::BoxFuture<'static, bool>
            }));

        let rx = self.browser_lifecycle_rx.write().await.take();
        let Some(mut rx) = rx else {
            tracing::debug!("browser lifecycle bridge already started; skipping");
            return;
        };
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let BrowserSessionLifecycleEvent { work_scope, active } = event;

                // Fan out to every live runtime handle whose conversation
                // resolves to this `WorkScope` (REQ-BROWSER-WS-002). A
                // worktree-scoped session is shared across continuation
                // members, so all of them need the lifecycle edge; a
                // conversation-scoped session affects only one runtime.
                //
                // Cost: O(N) `get_conversation` DB reads per event, N =
                // count of live runtime handles. Browser-session
                // lifecycle events are rare (create / kill / idle
                // cleanup), N is small (active runtimes only), and
                // `get_conversation` is a single-row indexed read — so
                // the absolute load is negligible. If this becomes a
                // hotspot, cache the scope on `ConversationHandle` at
                // get-or-create time (task 62008).
                let conv_ids: Vec<String> = {
                    let runtimes = manager.runtimes.read().await;
                    runtimes.keys().cloned().collect()
                };

                let mut delivered = 0usize;
                for conv_id in conv_ids {
                    let Ok(conv) = manager.db().get_conversation(&conv_id).await else {
                        continue;
                    };
                    let conv_scope = crate::work_scope::WorkScope::resolve(
                        &conv.id,
                        conv.conv_mode.worktree_path().map(std::path::Path::new),
                    );
                    if conv_scope != work_scope {
                        continue;
                    }
                    let broadcaster = {
                        let runtimes = manager.runtimes.read().await;
                        runtimes.get(&conv_id).map(|h| h.broadcast_tx.clone())
                    };
                    let Some(broadcaster) = broadcaster else {
                        continue;
                    };
                    if broadcaster
                        .send_seq(|seq| SseEvent::BrowserSessionState {
                            sequence_id: seq,
                            active,
                        })
                        .is_ok()
                    {
                        delivered += 1;
                    }
                }

                if delivered == 0 {
                    tracing::debug!(
                        work_scope = %work_scope,
                        active,
                        "dropping browser session lifecycle event — no live runtime handle on scope"
                    );
                }

                // A browser liveness edge is also a work-scope change
                // (REQ-WSUI-007). Forward the scope to the work-scope bridge
                // so it re-broadcasts a `WorkScopeUpdate` carrying the full
                // refreshed inventory. Reuses this bridge's scope resolution
                // rather than re-deriving it. Best-effort: a closed channel
                // (bridge not started) is logged at debug.
                if let Err(e) = manager.work_scope_browser_tx.send(work_scope.clone()) {
                    tracing::debug!(
                        work_scope = %work_scope,
                        error = %e,
                        "dropping work-scope forward of browser edge — channel closed"
                    );
                }
            }
            tracing::info!("Browser lifecycle bridge stopped");
        });
    }

    /// Start the bridge that turns bash state-transition signals, tmux
    /// state-transition signals, and forwarded browser liveness edges into
    /// per-conversation `WorkScopeUpdate` broadcasts (REQ-WSUI-007 /
    /// REQ-WSUI-008). Must be called once after `RuntimeManager::new`; a
    /// double call is a no-op.
    ///
    /// On each signal for `WorkScope` W it assembles W's full inventory from
    /// the three live registries (the same `assemble_inventory` the pull
    /// endpoint uses) and broadcasts it to W's conversation(s) using the
    /// identical scope-resolution the browser lifecycle bridge applies:
    /// enumerate live runtime handles, resolve each conversation's scope via
    /// `WorkScope::resolve`, match against W. REQ-PROJ-025 guarantees at most
    /// one non-terminal conversation per scope, so this lands on the one live
    /// member.
    pub async fn start_work_scope_bridge(self: &Arc<Self>) {
        let bash_rx = self.bash_lifecycle_rx.write().await.take();
        let tmux_rx = self.tmux_lifecycle_rx.write().await.take();
        let browser_rx = self.work_scope_browser_rx.write().await.take();
        let (Some(mut bash_rx), Some(mut tmux_rx), Some(mut browser_rx)) =
            (bash_rx, tmux_rx, browser_rx)
        else {
            tracing::debug!("work-scope bridge already started; skipping");
            return;
        };
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let work_scope = tokio::select! {
                    Some(event) = bash_rx.recv() => event.work_scope,
                    Some(event) = tmux_rx.recv() => event.work_scope,
                    Some(scope) = browser_rx.recv() => scope,
                    else => break,
                };
                manager.broadcast_work_scope_update(&work_scope).await;
            }
            tracing::info!("Work-scope bridge stopped");
        });
    }

    /// Assemble `work_scope`'s inventory and broadcast a `WorkScopeUpdate` to
    /// every live runtime handle whose conversation resolves to it. Factored
    /// out of [`Self::start_work_scope_bridge`] so the bash and browser signal
    /// arms share one routing path.
    pub(crate) async fn broadcast_work_scope_update(self: &Arc<Self>, work_scope: &WorkScope) {
        let inventory = phoenix_tools::work_scope_inventory::assemble_inventory(
            work_scope,
            self.bash_handles(),
            self.tmux_registry(),
            self.browser_sessions(),
        )
        .await;

        // Same resolution as the browser lifecycle bridge: enumerate live
        // runtime handles, resolve each conversation's scope, match.
        let conv_ids: Vec<String> = {
            let runtimes = self.runtimes.read().await;
            runtimes.keys().cloned().collect()
        };

        let mut delivered = 0usize;
        for conv_id in conv_ids {
            let Ok(conv) = self.db().get_conversation(&conv_id).await else {
                continue;
            };
            let conv_scope = crate::work_scope::WorkScope::resolve(
                &conv.id,
                conv.conv_mode.worktree_path().map(std::path::Path::new),
            );
            if &conv_scope != work_scope {
                continue;
            }
            let broadcaster = {
                let runtimes = self.runtimes.read().await;
                runtimes.get(&conv_id).map(|h| h.broadcast_tx.clone())
            };
            let Some(broadcaster) = broadcaster else {
                continue;
            };
            if broadcaster
                .send_seq(|seq| SseEvent::WorkScopeUpdate {
                    sequence_id: seq,
                    inventory: inventory.clone(),
                })
                .is_ok()
            {
                delivered += 1;
            }
        }

        if delivered == 0 {
            tracing::debug!(
                work_scope = %work_scope,
                "dropping work-scope update — no live runtime handle on scope"
            );
        }
    }

    /// Whether `work_scope` still owns a non-terminal conversation — the
    /// "is this scope live?" question the browser idle-cleanup hook asks.
    ///
    /// Authority is the DATABASE, not the live-runtime-handle set: a
    /// conversation can be non-terminal in the DB yet carry no runtime handle
    /// (after a server restart, or runtime eviction). For a
    /// `WorkScope::Worktree(path)` we query the conversations whose
    /// `conv_mode.worktree_path` is that path; for a `WorkScope::Conversation`
    /// only that conversation resolves to the scope. Each candidate's scope is
    /// resolved via `WorkScope::resolve` and matched against `work_scope`. A
    /// match counts as live only when its conversation is neither terminal
    /// (`ConvState::is_terminal`) nor archived. REQ-PROJ-025 guarantees at
    /// most one non-terminal conversation per scope, so the first match is
    /// decisive.
    ///
    /// A genuinely absent conversation row (`DbError::ConversationNotFound`)
    /// is `Ok(false)` — that is a definitive "not live", not a failure. Any
    /// other DB error propagates as `Err`: liveness is unknowable, and each
    /// caller picks its own policy (the idle reaper maps `Err` to "live" to
    /// avoid premature teardown; the cleanup cascade fails the operation
    /// rather than archive while skipping resource teardown).
    pub(crate) async fn scope_has_live_conversation(
        &self,
        work_scope: &WorkScope,
    ) -> Result<bool, crate::db::DbError> {
        self.scope_has_live_conversation_inner(work_scope, None)
            .await
    }

    /// Like [`scope_has_live_conversation`] but skips `excluded_conv_id` when
    /// enumerating. Used by the resource-cleanup cascade to ask "does a
    /// non-terminal, non-archived conversation OTHER THAN the one being
    /// deleted still resolve to this scope?"
    ///
    /// Exclusion is load-bearing: the cascade runs BEFORE the terminal-state
    /// write, so the conversation being deleted/archived still reads
    /// non-terminal in the DB. Without excluding it, the scope would always
    /// look live and never tear down.
    pub(crate) async fn scope_has_live_conversation_excluding(
        &self,
        work_scope: &WorkScope,
        excluded_conv_id: &str,
    ) -> Result<bool, crate::db::DbError> {
        self.scope_has_live_conversation_inner(work_scope, Some(excluded_conv_id))
            .await
    }

    async fn scope_has_live_conversation_inner(
        &self,
        work_scope: &WorkScope,
        excluded_conv_id: Option<&str>,
    ) -> Result<bool, crate::db::DbError> {
        // Candidate conversations come from the DB so a non-terminal owner
        // without a runtime handle (post-restart / post-eviction) still
        // counts. A `WorkScope::Conversation(id)` is single-owner: only `id`
        // resolves to it, so we look up just that conversation. A
        // `WorkScope::Worktree(path)` can be shared (a Work sub-agent and its
        // parent), so we query every conversation on that worktree path.
        let candidates: Vec<crate::db::Conversation> = match work_scope {
            WorkScope::Conversation(id) => {
                if excluded_conv_id == Some(id.as_str()) {
                    return Ok(false);
                }
                match self.db().get_conversation(id).await {
                    Ok(conv) => vec![conv],
                    // A genuinely absent row is a definitive "not live", not a
                    // failure. Any other DB error is unknowable liveness and
                    // propagates so each caller picks its own policy.
                    Err(crate::db::DbError::ConversationNotFound(_)) => return Ok(false),
                    Err(e) => return Err(e),
                }
            }
            WorkScope::Worktree(path) => self.db().list_conversations_for_worktree(path).await?,
            // The `Global` singleton scope (the `/new` page global terminal)
            // is not owned by any conversation — `WorkScope::resolve` only ever
            // yields `Worktree` or `Conversation` — so no conversation can
            // preserve it.
            WorkScope::Global => return Ok(false),
        };

        for conv in candidates {
            if excluded_conv_id == Some(conv.id.as_str()) {
                continue;
            }
            // An archived conversation is not a live owner even when its
            // row still reads non-terminal: archiving a Work/Branch chain
            // archives earlier members before the leaf's cleanup runs.
            // Counting it as live would preserve the shared scope and leak
            // its bash/tmux/browser/terminal resources.
            if conv.archived {
                continue;
            }
            if !self.conv_is_scope_owner(&conv, excluded_conv_id).await? {
                continue;
            }
            let conv_scope = crate::work_scope::WorkScope::resolve(
                &conv.id,
                conv.conv_mode.worktree_path().map(std::path::Path::new),
            );
            if &conv_scope == work_scope {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether `conv` is a live owner of its work scope — the predicate that
    /// must agree with `reconcile_worktrees` so every cleanup path makes the
    /// same "is this worktree still owned?" decision (REQ-BED-030/031).
    ///
    /// A conversation owns its scope when it is:
    ///   - non-terminal (an in-flight conversation, or one idle/awaiting input
    ///     after a restart with no runtime handle); or
    ///   - `ContextExhausted` that has NOT been continued
    ///     (`continued_in_conv_id IS NULL`) — the worktree is deliberately
    ///     preserved pending the user's Continue / Abandon / `MarkAsMerged`
    ///     decision, so it is an owner; or
    ///   - `HandedOff` whose continuation chain dead-ends with nothing live —
    ///     the row that handed off normally cedes ownership to its live
    ///     continuation, but if the whole forward chain is gone (every member
    ///     terminal/archived) the `HandedOff` row is the last protector of the
    ///     preserved worktree.
    ///
    /// A `ContextExhausted` row that HAS been continued is NOT an owner: the
    /// user explicitly chose Continue, transferring ownership to the
    /// continuation (the leaf), and abandoning that leaf is an explicit
    /// destroy intent that must tear the shared worktree down. The parent is
    /// already gated from terminal actions once `continued_in_conv_id` is set,
    /// so it must not block the leaf's cleanup — defer entirely to the
    /// continuation chain, owning the scope only if some non-excluded member
    /// downstream is still live.
    ///
    /// `Completed` / `Failed` / `Terminal` are never owners.
    async fn conv_is_scope_owner(
        &self,
        conv: &crate::db::Conversation,
        excluded_conv_id: Option<&str>,
    ) -> Result<bool, crate::db::DbError> {
        use phoenix_core::domain::sm_state::ConvState;
        match &conv.state {
            // A ContextExhausted row that has NOT been continued owns the
            // preserved worktree unconditionally (pending the user's
            // Continue / Abandon / MarkAsMerged decision).
            ConvState::ContextExhausted { .. } if conv.continued_in_conv_id.is_none() => Ok(true),
            // ContextExhausted that HAS been continued cedes ownership to the
            // continuation chain: it owns the scope only while some downstream
            // member is still live. Unlike HandedOff it is NOT a dead-end
            // protector — once the continuation is gone (terminal or the very
            // leaf being cleaned up), the user's explicit Continue→Abandon
            // intent is to tear the shared worktree down.
            ConvState::ContextExhausted { .. } => Ok(self
                .continuation_chain_has_live_owner(&conv.id, excluded_conv_id)
                .await?),
            // HandedOff owns the worktree only when its continuation chain has
            // gone dead — otherwise the live continuation is the owner and is
            // counted on its own. When the live continuation is the row being
            // cleaned up (excluded), ownership hands back to this HandedOff
            // predecessor (REQ-BED-031, the 61003 chain-handback intent).
            ConvState::HandedOff { .. } => Ok(!self
                .continuation_chain_has_live_owner(&conv.id, excluded_conv_id)
                .await?),
            // Other terminal states (Completed / Failed / Terminal) are not owners.
            s if s.is_terminal() => Ok(false),
            // Non-terminal: a live owner (in-flight or handle-less post-restart).
            _ => Ok(true),
        }
    }

    /// Whether any conversation STRICTLY DOWNSTREAM of `conv_id` on the
    /// continuation chain (`continued_in_conv_id` edges) is itself a live scope
    /// owner. Two callers read it with opposite polarity:
    ///   - a continued `ContextExhausted` row owns the scope IFF this is true
    ///     (it has ceded to its continuation and only "borrows" liveness back
    ///     from a live downstream member);
    ///   - a `HandedOff` row owns the scope IFF this is false (it is the
    ///     dead-end protector when the whole forward chain has gone).
    ///
    /// `excluded_conv_id` is treated as not-live: the cleanup cascade runs
    /// BEFORE the deleted conversation's terminal-state write, so a deletion of
    /// the live continuation must not let that doomed row keep the chain
    /// "alive".
    ///
    /// `chain_members_forward` already flattens the ENTIRE forward chain
    /// (`conv_id` and all transitive `continued_in_conv_id` successors) into a
    /// single depth-ordered list, so multi-hop chains (A→B→C…) are handled by
    /// iterating that flat list — no per-member recursion is needed, and
    /// termination is guaranteed by the finite list the recursive CTE returns.
    /// Each member is scored by the SAME single-node rule as
    /// `conv_is_scope_owner` so the two predicates never disagree:
    ///   - a non-terminal (non-archived, non-excluded) member is live;
    ///   - a NON-continued `ContextExhausted`/`HandedOff` member is live (the
    ///     worktree-preservation / dead-end protector owner);
    ///   - a CONTINUED `ContextExhausted`/`HandedOff` member is NOT live on its
    ///     own — it has ceded to ITS downstream chain, whose members already
    ///     appear later in this same flattened list, so its liveness is decided
    ///     there.
    async fn continuation_chain_has_live_owner(
        &self,
        conv_id: &str,
        excluded_conv_id: Option<&str>,
    ) -> Result<bool, crate::db::DbError> {
        use phoenix_core::domain::sm_state::ConvState;
        let chain = self.db().chain_members_forward(conv_id).await?;
        // `chain[0]` is `conv_id` itself (the row that ceded ownership); skip
        // it and inspect only its successors.
        for member_id in chain.into_iter().skip(1) {
            if excluded_conv_id == Some(member_id.as_str()) {
                continue;
            }
            let member = match self.db().get_conversation(&member_id).await {
                Ok(m) => m,
                Err(crate::db::DbError::ConversationNotFound(_)) => continue,
                Err(e) => return Err(e),
            };
            if member.archived {
                continue;
            }
            let is_live_owner = match &member.state {
                // A non-continued ContextExhausted/HandedOff member is the
                // worktree-preservation / dead-end protector owner. A CONTINUED
                // one cedes to its own downstream chain — and since
                // `chain_members_forward` already flattened those successors
                // into this list, their liveness is scored on their own rows,
                // not borrowed up to this intermediate node. This is the
                // multi-hop fix: an intermediate B in A→B→C that was itself
                // continued (to C) must NOT count as live just for being
                // ContextExhausted when C (its only live-relevant successor) is
                // gone/excluded.
                ConvState::ContextExhausted { .. } | ConvState::HandedOff { .. } => {
                    member.continued_in_conv_id.is_none()
                }
                // Other terminal states (Completed / Failed / Terminal) never own.
                s if s.is_terminal() => false,
                // Non-terminal: a live owner (in-flight or handle-less post-restart).
                _ => true,
            };
            if is_live_owner {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Start the background task that handles sub-agent spawn/cancel requests
    /// Must be called once after creating the `RuntimeManager`
    pub async fn start_sub_agent_handler(self: &Arc<Self>) {
        let manager = Arc::clone(self);

        // Take the receivers (can only be done once)
        let spawn_rx = self.spawn_rx.write().await.take();
        let cancel_rx = self.cancel_rx.write().await.take();
        let handoff_rx = self.handoff_rx.write().await.take();
        let fork_cmd_rx = self.fork_cmd_rx.write().await.take();

        if let (
            Some(mut spawn_rx),
            Some(mut cancel_rx),
            Some(mut handoff_rx),
            Some(mut fork_cmd_rx),
        ) = (spawn_rx, cancel_rx, handoff_rx, fork_cmd_rx)
        {
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(req) = spawn_rx.recv() => {
                            manager.handle_spawn_request(req).await;
                        }
                        Some(req) = cancel_rx.recv() => {
                            manager.handle_cancel_request(req).await;
                        }
                        Some(req) = handoff_rx.recv() => {
                            manager.handle_task_handoff_request(req).await;
                        }
                        // Single serialized fork-resolution consumer: each command
                        // is handled to completion before the next is taken, so two
                        // fork critical sections cannot interleave (REQ-PROJ-034/035).
                        Some(cmd) = fork_cmd_rx.recv() => {
                            manager.handle_fork_command(cmd).await;
                        }
                        else => break,
                    }
                }
                tracing::info!("Sub-agent/task-handoff/fork-resolution handler stopped");
            });
        }
    }

    async fn handle_task_handoff_request(self: &Arc<Self>, req: TaskApprovalHandoffRequest) {
        let result = self.create_and_start_task_handoff(&req).await;
        let _ = req.response_tx.send(result);
    }

    async fn create_and_start_task_handoff(
        self: &Arc<Self>,
        req: &TaskApprovalHandoffRequest,
    ) -> Result<TaskApprovalHandoffResponse, String> {
        let successor = self
            .db
            .create_task_approval_handoff_conversation(&req.parent_conversation_id, &req.approval)
            .await
            .map_err(|e| e.to_string())?;
        let _ = self.get_or_create(&successor.id).await?;
        Ok(TaskApprovalHandoffResponse {
            successor_conv_id: successor.id,
        })
    }

    /// Handle a sub-agent spawn request
    #[allow(clippy::too_many_lines)]
    async fn handle_spawn_request(self: &Arc<Self>, req: SubAgentSpawnRequest) {
        let SubAgentSpawnRequest {
            spec,
            parent_conversation_id,
            parent_event_tx,
        } = req;

        tracing::info!(
            agent_id = %spec.agent_id,
            parent_id = %parent_conversation_id,
            task = %spec.task,
            "Spawning sub-agent"
        );

        // 1. Look up parent conversation to inherit its conv_mode
        let parent_conv = match self.db.get_conversation(&parent_conversation_id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to look up parent conversation");
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id: spec.agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: format!("Failed to look up parent conversation: {e}"),
                            error_kind: crate::db::ErrorKind::SubAgentError,
                        },
                    })
                    .await;
                return;
            }
        };

        // Derive sub-agent conv_mode from spec.mode + parent's mode.
        // Explore sub-agents are always Explore. Work sub-agents inherit
        // the parent's Work mode (branch, base_branch, worktree_path).
        let sub_conv_mode = match spec.mode {
            SubAgentMode::Explore => ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
            SubAgentMode::Work => parent_conv.conv_mode.clone(),
        };

        let spec_cwd = match crate::conversation_cwd::validate_conversation_cwd(&spec.cwd) {
            Ok(cwd) => cwd,
            Err(e) => {
                tracing::warn!(agent_id = %spec.agent_id, cwd = %spec.cwd, error = %e, "Rejected sub-agent spawn with invalid cwd");
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id: spec.agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: format!("Invalid sub-agent working directory: {e}"),
                            error_kind: crate::db::ErrorKind::SubAgentError,
                        },
                    })
                    .await;
                return;
            }
        };

        // 2. Create conversation in DB with correct conv_mode
        let slug = format!("sub-{}", spec.agent_id.get(..8).unwrap_or(&spec.agent_id));
        let conv = match self
            .db
            .create_conversation_with_project(
                &spec.agent_id,
                &slug,
                spec_cwd.raw(),
                false, // user_initiated = false
                Some(&parent_conversation_id),
                Some(&spec.model_id), // inherit parent's model
                None,                 // project_id
                &sub_conv_mode,
                None,                     // desired_base_branch
                None, // seed_parent_id (sub-agents use `parent_conversation_id` above)
                None, // seed_label
                parent_conv.llm_language, // inherit language from parent
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create sub-agent conversation");
                // Notify parent of failure
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id: spec.agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: format!("Failed to create conversation: {e}"),
                            error_kind: crate::db::ErrorKind::SubAgentError,
                        },
                    })
                    .await;
                return;
            }
        };

        // Persist the named-agent persona (REQ-AG-006) so a sub-agent runtime
        // recreated mid-run (e.g. model-upgrade eviction) keeps it instead of
        // falling back to the generic prompt. Best-effort: the live
        // conv_context built below carries the persona regardless, so a write
        // failure only degrades a subsequent resume — logged, not fatal.
        if let Some(persona) = spec.persona.as_deref() {
            if let Err(e) = self.db.set_sub_agent_persona(&conv.id, persona).await {
                tracing::warn!(
                    error = %e,
                    conv_id = %conv.id,
                    "Failed to persist sub-agent persona; a resumed runtime would fall back to the generic prompt"
                );
            }
        }

        // 2. Insert initial task as synthetic user message
        let message_id = uuid::Uuid::new_v4().to_string();
        let content = crate::db::MessageContent::user(&spec.task);
        if let Err(e) = self
            .db
            .add_message(&message_id, &conv.id, &content, None, None)
            .await
        {
            tracing::error!(error = %e, "Failed to add initial message");
            let _ = parent_event_tx
                .send(Event::SubAgentResult {
                    agent_id: spec.agent_id,
                    outcome: SubAgentOutcome::Failure {
                        error: format!("Failed to add initial message: {e}"),
                        error_kind: crate::db::ErrorKind::SubAgentError,
                    },
                })
                .await;
            return;
        }

        // 3. Create sub-agent context with max_turns from spec (REQ-PROJ-008)
        let root_conversation_id =
            find_root_conversation_id(&self.db, &parent_conversation_id).await;
        let context_window = self.llm_registry.context_window(&spec.model_id);
        let mut conv_context = ConvContext::sub_agent(
            &conv.id,
            spec_cwd.path_buf(),
            &spec.model_id,
            context_window,
            root_conversation_id,
        );
        conv_context.max_turns = spec.max_turns;
        conv_context.mode_context = Some(conv_mode_to_context(&sub_conv_mode));
        conv_context.explore_bash = ExploreToolPolicy::from_platform(&self.platform).bash();
        conv_context.mode = match &sub_conv_mode {
            ConvMode::Direct => ModeKind::Direct,
            ConvMode::Explore { .. } | ConvMode::Work { .. } => ModeKind::Managed,
            ConvMode::Branch { .. } => ModeKind::Branch,
        };
        // Scope keying derives from the persisted conv_mode's worktree path,
        // the same authority every DB-facing path uses. An Explore sub-agent
        // has `worktree_path: None`, so it scopes to its own
        // `WorkScope::Conversation(id)` — isolated from the parent, matching
        // the inventory / cleanup / SSE derivations.
        conv_context.work_scope_worktree = sub_conv_mode.worktree_path().map(PathBuf::from);
        // Sub-agent inherits parent's worktree cwd; discover the project's
        // tasks directory the same way the parent did.
        conv_context.tasks_dir_name =
            taskmd_core::discover::discover_or_default(&conv_context.working_dir)
                .to_string_lossy()
                .into_owned();
        // Sub-agents inherit their parent's LLM language.
        conv_context.llm_language = conv.llm_language;
        // Named-agent persona (REQ-AG-006): replaces the base preamble in the
        // sub-agent's system prompt. `None` for anonymous spawns.
        conv_context.persona = spec.persona.clone();

        // 4. Create channels for the sub-agent runtime. The broadcaster
        // seeds its counter from the message we just inserted (sequence_id=1)
        // so the first non-message event is ordered strictly after it.
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcaster = SseBroadcaster::new(SSE_BROADCAST_CAPACITY, 1);

        // 5. Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), spec.model_id.clone());
        // Select tool registry based on sub-agent mode (REQ-PROJ-008).
        // Sub-agents get MCP access via the parent's MCP manager.
        let explore_policy = ExploreToolPolicy::from_platform(&self.platform);
        let registry = match spec.mode {
            SubAgentMode::Explore => ToolRegistry::for_subagent_explore(explore_policy),
            SubAgentMode::Work => ToolRegistry::for_subagent_work(),
        };
        // Sub-agents cannot spawn, so they carry an empty agent catalog.
        let tool_executor = ToolRegistryExecutor::with_mcp(
            registry,
            self.mcp_manager.clone(),
            Arc::from(Vec::new()),
        );

        // 6. Create runtime with parent notification
        let runtime: ProductionRuntime = ConversationRuntime::new(
            conv_context,
            ConvState::Idle,
            storage,
            llm_client,
            tool_executor,
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.tmux_registry.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            event_rx,
            event_tx.clone(),
            broadcaster.clone(),
        )
        .with_parent(parent_event_tx.clone())
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone())
        .with_task_handoff_channel(self.handoff_tx.clone())
        .with_credential_helper(self.credential_helper.clone());

        // Live-state watch channel for sub-agent (seeded Idle; transitions publish updates).
        let (sub_state_tx, sub_state_rx) = watch::channel(ConvState::Idle);
        let runtime = runtime.with_state_watcher(sub_state_tx);

        // 7. Store handle
        let sub_agent_identity = Arc::new(());
        self.runtimes.write().await.insert(
            conv.id.clone(),
            ConversationHandle {
                event_tx: event_tx.clone(),
                broadcast_tx: broadcaster.clone(),
                identity: sub_agent_identity.clone(),
                state_rx: sub_state_rx,
            },
        );

        // 8. Set up per-agent timeout — sends UserCancel if sub-agent exceeds its limit.
        // This is a safety net; the parent's AwaitingSubAgents deadline is the primary
        // enforcement (REQ-SA-006). Both fire independently.
        let timeout_duration = spec.timeout;
        let timeout_task = {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout_duration).await;
                tracing::info!("Sub-agent timeout reached, sending cancel");
                let _ = event_tx
                    .send(Event::UserCancel {
                        reason: Some("Sub-agent timed out".to_string()),
                        cause: crate::state_machine::event::CancelCause::Timeout,
                    })
                    .await;
            })
        };

        // 9. Start runtime task
        let conv_id = conv.id.clone();
        let task_text = spec.task.clone();
        let manager_for_cleanup = Arc::clone(self);
        tokio::spawn(async move {
            // Send initial UserMessage event to start the conversation
            // Sub-agents generate their own message_id since they don't have a client
            let _ = event_tx
                .send(Event::UserMessage {
                    text: task_text,
                    llm_text: None, // Sub-agent tasks are already fully specified
                    images: vec![],
                    files: vec![],
                    message_id: uuid::Uuid::new_v4().to_string(),
                    user_agent: Some("Phoenix Sub-Agent".to_string()),
                    skill_invocation: None,
                })
                .await;

            runtime.run().await;

            // Cancel timeout — sub-agent finished before its limit
            timeout_task.abort();

            // Only remove this sub-agent's entry. The identity check guards
            // against the (unlikely) case where a replacement was inserted
            // under the same key between run() finishing and this write lock.
            let mut runtimes = manager_for_cleanup.runtimes.write().await;
            if runtimes
                .get(&conv_id)
                .is_some_and(|h| Arc::ptr_eq(&h.identity, &sub_agent_identity))
            {
                runtimes.remove(&conv_id);
                tracing::info!(conv_id = %conv_id, "Sub-agent runtime finished and cleaned up");
            } else {
                tracing::debug!(
                    conv_id = %conv_id,
                    "Sub-agent cleanup: entry was replaced, skipping remove"
                );
            }
        });
    }

    /// Handle a sub-agent cancel request
    async fn handle_cancel_request(&self, req: SubAgentCancelRequest) {
        let SubAgentCancelRequest {
            ids,
            parent_conversation_id: _,
            parent_event_tx,
        } = req;

        let runtimes = self.runtimes.read().await;

        for agent_id in ids {
            if let Some(handle) = runtimes.get(&agent_id) {
                tracing::info!(agent_id = %agent_id, "Sending cancel to sub-agent");
                let _ = handle
                    .event_tx
                    .send(Event::UserCancel {
                        reason: None,
                        cause: crate::state_machine::event::CancelCause::UserRequested,
                    })
                    .await;
            } else {
                // Runtime not found - synthesize failure result
                tracing::warn!(agent_id = %agent_id, "Sub-agent runtime not found, synthesizing failure");
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: "Sub-agent runtime not found".to_string(),
                            error_kind: crate::db::ErrorKind::Cancelled,
                        },
                    })
                    .await;
            }
        }
    }

    /// Get or create a runtime for a conversation
    #[allow(clippy::too_many_lines)]
    pub async fn get_or_create(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<ConversationHandle, String> {
        // Check if already running
        {
            let runtimes = self.runtimes.read().await;
            if let Some(handle) = runtimes.get(conversation_id) {
                return Ok(ConversationHandle {
                    event_tx: handle.event_tx.clone(),
                    broadcast_tx: handle.broadcast_tx.clone(),
                    identity: handle.identity.clone(),
                    state_rx: handle.state_rx.clone(),
                });
            }
        }

        // Need to start a new runtime
        let conv = self
            .db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let conv_cwd = crate::conversation_cwd::validate_conversation_cwd_for_runtime(
            conversation_id,
            &conv.cwd,
        )
        .map_err(|e| {
            format!("Conversation '{conversation_id}' has an invalid working directory: {e}")
        })?;

        // Check if this is a sub-agent being resumed (shouldn't happen normally)
        let is_sub_agent = conv.parent_conversation_id.is_some();

        // Resolve model once: use conversation's stored model, or fall back to registry default
        let model_id = conv
            .model
            .clone()
            .unwrap_or_else(|| self.llm_registry.default_model_id().to_string());
        let context_window = self.llm_registry.context_window(&model_id);
        let mode_context = conv_mode_to_context(&conv.conv_mode);
        let mut context = if is_sub_agent {
            let root_id = find_root_conversation_id(&self.db, conversation_id).await;
            ConvContext::sub_agent(
                &conv.id,
                conv_cwd.path_buf(),
                &model_id,
                context_window,
                root_id,
            )
        } else {
            ConvContext::new(&conv.id, conv_cwd.path_buf(), &model_id, context_window)
        };
        context.mode_context = Some(mode_context);
        context.explore_bash = ExploreToolPolicy::from_platform(&self.platform).bash();
        context.desired_base_branch = conv.desired_base_branch.clone();
        context.mode = match &conv.conv_mode {
            ConvMode::Direct => ModeKind::Direct,
            ConvMode::Explore { .. } | ConvMode::Work { .. } => ModeKind::Managed,
            ConvMode::Branch { .. } => ModeKind::Branch,
        };
        // Scope keying derives from the persisted conv_mode's worktree path,
        // the single authority every DB-facing path uses for
        // `WorkScope::resolve`. Keeps `ToolContext.work_scope` in lock-step
        // with the inventory / cleanup / SSE scope derivations.
        context.work_scope_worktree = conv.conv_mode.worktree_path().map(PathBuf::from);
        // Discover the project's tasks directory once at conversation
        // startup; cached for the lifetime of this runtime so state machine,
        // executor, patch tool registration, and system prompt all agree on
        // the same name without re-walking the worktree.
        context.tasks_dir_name = taskmd_core::discover::discover_or_default(&context.working_dir)
            .to_string_lossy()
            .into_owned();
        // Pin the LLM-facing language for the lifetime of this runtime so
        // the system prompt and tool descriptions stay consistent across all
        // turns even if the global default changes mid-conversation.
        context.llm_language = conv.llm_language;
        // Restore a named-agent persona (REQ-AG-006) on the resume path: it is
        // set at spawn on the fresh ConvContext, but a runtime recreated mid-run
        // (e.g. model-upgrade eviction) rebuilds the context from the DB, so the
        // persona must be re-read here or remaining turns lose it.
        if is_sub_agent {
            match self.db.get_sub_agent_persona(conversation_id).await {
                Ok(persona) => context.persona = persona,
                Err(e) => tracing::warn!(
                    error = %e,
                    conv_id = %conversation_id,
                    "Failed to read sub-agent persona on resume; falling back to the generic prompt"
                ),
            }
        }

        let (event_tx, event_rx) = mpsc::channel(32);
        // Inherit the broadcaster from an eviction if available (e.g. model
        // upgrade). This keeps existing SSE clients subscribed to the same
        // channel so they receive events from the new runtime without needing
        // to reconnect. If no evicted broadcaster exists, create a fresh one
        // seeded from the DB's highest sequence_id to avoid collisions.
        let broadcaster = {
            let mut evicted = self.evicted_broadcasters.write().await;
            if let Some(b) = evicted.remove(conversation_id) {
                tracing::debug!(
                    conv_id = %conversation_id,
                    receivers = b.receiver_count(),
                    "New runtime inheriting evicted broadcaster; SSE clients stay connected"
                );
                b
            } else {
                let initial_last_seq = self
                    .db
                    .get_last_sequence_id(conversation_id)
                    .await
                    .unwrap_or(0);
                SseBroadcaster::new(SSE_BROADCAST_CAPACITY, initial_last_seq)
            }
        };

        // Consume any recorded model-upgrade eviction for this conversation.
        // Drives the wording of the auto-continue recovery message below
        // (task 02710).
        let evicted_for_model_upgrade = self
            .evicted_model_upgrades
            .write()
            .await
            .remove(conversation_id);

        // Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), model_id);

        // Tool registry selection -- sub-agents get a restricted tool set
        // (no spawn_agents, no ask_user_question, no skill); parent
        // conversations get the mode-appropriate registry. Both layers wrap
        // their registry with `with_mcp` so MCP tool defs resolve live from
        // the manager on every `definitions()` call.
        // Freeze the named-agent catalog once per conversation so the
        // spawn_agents schema and the executor's agent_type resolution share a
        // single catalog instead of independently re-discovering the filesystem
        // (REQ-AG-008). Sub-agents cannot spawn, so theirs is empty.
        let agent_catalog: Arc<[phoenix_agents::AgentDefinition]> = if is_sub_agent {
            Arc::from(Vec::new())
        } else {
            Arc::from(phoenix_agents::discover_agents(&context.working_dir))
        };
        let tool_executor = if is_sub_agent {
            let registry = sub_agent_registry_for_conv_mode(
                &conv.conv_mode,
                ExploreToolPolicy::from_platform(&self.platform),
            );
            ToolRegistryExecutor::with_mcp(
                registry,
                self.mcp_manager.clone(),
                agent_catalog.clone(),
            )
        } else {
            use crate::db::ConvMode;
            let registry = match conv.conv_mode {
                ConvMode::Explore { .. } => ToolRegistry::explore(
                    &context.tasks_dir_name,
                    agent_catalog.to_vec(),
                    ExploreToolPolicy::from_platform(&self.platform),
                ),
                ConvMode::Direct => {
                    // Full tool suite for Direct mode. `propose_task` (the
                    // fork proposal) is offered only when the working dir is
                    // inside a git repo — a fork cuts from the repository's
                    // default branch (REQ-PROJ-036).
                    let registry = ToolRegistry::direct(agent_catalog.to_vec());
                    if phoenix_core::git::detect_git_repo_root(&context.working_dir).is_some() {
                        registry.with_propose_task().with_commission_review()
                    } else {
                        registry
                    }
                }
                ConvMode::Work { .. } | ConvMode::Branch { .. } => {
                    // Full tool suite plus `propose_task` (non-blocking fork
                    // proposal — REQ-PROJ-036). Work/Branch always sit on git
                    // history, so the tool is always offered.
                    ToolRegistry::direct(agent_catalog.to_vec())
                        .with_propose_task()
                        .with_commission_review()
                }
            };
            ToolRegistryExecutor::with_mcp(
                registry,
                self.mcp_manager.clone(),
                agent_catalog.clone(),
            )
        };

        // Determine initial state: check if conversation needs auto-continuation
        // REQ-BED-007 says resume from idle, but we need to handle interrupted turns
        let (initial_state, initial_state_updated_at, needs_auto_continue) =
            self.determine_resume_state(conversation_id).await?;

        // Seed the executor's in-memory steering queue from the normalized
        // steering_messages tables.
        let steering_queue = self
            .db
            .get_steering_queue(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let runtime: ProductionRuntime = ConversationRuntime::new(
            context,
            initial_state.clone(),
            storage,
            llm_client,
            tool_executor,
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.tmux_registry.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            event_rx,
            event_tx.clone(),
            broadcaster.clone(),
        )
        .with_state_updated_at(initial_state_updated_at)
        .with_steering_queue(steering_queue)
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone())
        .with_task_handoff_channel(self.handoff_tx.clone())
        .with_credential_helper(self.credential_helper.clone())
        .with_agent_catalog(agent_catalog);

        // Fork proposals are bound to top-level (parent) origins; sub-agents
        // never hold any. Give parent runtimes the fork-resolution consumer
        // sender so a terminal transition retires their still-pending proposals
        // through the serialized consumer (REQ-PROJ-035).
        let runtime = if is_sub_agent {
            runtime
        } else {
            runtime.with_fork_command_sender(self.fork_cmd_tx.clone())
        };

        // Create the live-state watch channel seeded with the initial state.
        // The executor writes to `state_tx` on every transition; the handle
        // exposes `state_rx` to HTTP handlers via `effective_conversation_state`.
        let (state_tx, state_rx) = watch::channel(initial_state);
        let runtime = runtime.with_state_watcher(state_tx);

        // If auto-continuing, inject a system message so the LLM knows a restart
        // happened. This also serves as the restart loop counter — recovery.rs
        // counts consecutive restart system messages at the tail of the history.
        if needs_auto_continue {
            use crate::db::SystemContent;
            use crate::runtime::recovery::RESTART_SYSTEM_MESSAGE_MARKER;

            // Keep the marker prefix regardless of cause: recovery.rs counts
            // consecutive marker messages at the tail as the restart-loop
            // guard. Only the human-readable cause differs (task 02710).
            let restart_msg = if evicted_for_model_upgrade {
                format!(
                    "{RESTART_SYSTEM_MESSAGE_MARKER} This conversation was resumed \
                     because its model was upgraded. The last tool execution was \
                     interrupted by the model switch. Review the tool results \
                     above before deciding what to do next. Do NOT re-execute the \
                     same command that was just running."
                )
            } else {
                format!(
                    "{RESTART_SYSTEM_MESSAGE_MARKER} This conversation was interrupted \
                     by a server restart. The last tool execution may have caused the \
                     restart. Review the tool results above before deciding what to do \
                     next. Do NOT re-execute the same command that was just running."
                )
            };
            let msg_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = self
                .db
                .add_message(
                    &msg_id,
                    conversation_id,
                    &crate::db::MessageContent::System(SystemContent { text: restart_msg }),
                    None,
                    None,
                )
                .await
            {
                tracing::warn!(conv_id = %conversation_id, error = %e,
                    "Failed to inject restart system message");
            }
            tracing::info!(conv_id = %conversation_id, "Will auto-continue interrupted conversation");
        }

        // Start runtime in background
        let conv_id = conversation_id.to_string();
        let manager_for_cleanup = Arc::clone(self);
        // Unique token for this runtime instance. Cleanup guards against
        // removing a replacement entry created after eviction.
        let identity = Arc::new(());
        let cleanup_identity = identity.clone();
        tokio::spawn(async move {
            runtime.run().await;

            // Only remove this runtime's HashMap entry. After evict_runtime()
            // a new runtime may have been inserted under the same key; we must
            // not evict that replacement.
            let mut runtimes = manager_for_cleanup.runtimes.write().await;
            if runtimes
                .get(&conv_id)
                .is_some_and(|h| Arc::ptr_eq(&h.identity, &cleanup_identity))
            {
                runtimes.remove(&conv_id);
                tracing::info!(conv_id = %conv_id, "Conversation runtime finished and cleaned up");
            } else {
                tracing::debug!(
                    conv_id = %conv_id,
                    "Runtime cleanup: entry replaced after eviction, skipping remove"
                );
            }
        });

        let handle = ConversationHandle {
            event_tx: event_tx.clone(),
            broadcast_tx: broadcaster.clone(),
            identity: identity.clone(),
            state_rx: state_rx.clone(),
        };

        // Store handle
        self.runtimes.write().await.insert(
            conversation_id.to_string(),
            ConversationHandle {
                event_tx,
                broadcast_tx: broadcaster,
                identity,
                state_rx,
            },
        );

        Ok(handle)
    }

    /// Inject a fake live handle carrying a specific `ConvState` into the
    /// handle map.  Test-only: simulates the post-restart auto-resume window
    /// where the executor has entered a transient state that has not yet been
    /// persisted to the DB row.
    #[cfg(test)]
    pub(crate) async fn inject_handle_for_test(&self, conv_id: &str, live_state: ConvState) {
        let (event_tx, event_rx) = mpsc::channel(32);
        // Keep the receiver alive so sends into event_tx succeed (a dropped
        // receiver closes the channel and causes `enqueue_steer_message` to err).
        tokio::spawn(async move {
            let mut rx = event_rx;
            while rx.recv().await.is_some() {}
        });
        let (_state_tx, state_rx) = watch::channel(live_state);
        self.runtimes.write().await.insert(
            conv_id.to_string(),
            ConversationHandle {
                event_tx,
                broadcast_tx: SseBroadcaster::new(SSE_BROADCAST_CAPACITY, 0),
                identity: Arc::new(()),
                state_rx,
            },
        );
    }

    /// Evict an active runtime so it gets recreated with fresh config on next access.
    /// Used after model upgrades to pick up the new model and context window.
    ///
    /// The old broadcaster is preserved in `evicted_broadcasters` so the new
    /// runtime can inherit it — existing SSE clients remain subscribed to the
    /// same channel without needing to reconnect. A `Shutdown` event is sent
    /// to the old runtime so it exits cleanly and releases its broadcaster
    /// clone, completing the hand-off.
    pub async fn evict_runtime(&self, conversation_id: &str, reason: EvictionReason) {
        let old = {
            let mut runtimes = self.runtimes.write().await;
            runtimes.remove(conversation_id)
        };

        // Record the cause unconditionally (even if no runtime was live): the
        // next get_or_create recreates the conversation with the new model and
        // its recovery message should name the real cause (task 02710).
        match reason {
            EvictionReason::ModelUpgrade => {
                self.evicted_model_upgrades
                    .write()
                    .await
                    .insert(conversation_id.to_string());
            }
        }

        if let Some(handle) = old {
            let receivers = handle.broadcast_tx.receiver_count();
            // Preserve broadcaster for the incoming runtime. The new runtime
            // inherits it in get_or_create so SSE clients stay connected.
            self.evicted_broadcasters
                .write()
                .await
                .insert(conversation_id.to_string(), handle.broadcast_tx);

            // Signal old runtime to exit cleanly. It drops its broadcaster
            // clone on exit, completing the reference hand-off.
            let _ = handle.event_tx.send(Event::Shutdown).await;
            tracing::info!(
                conv_id = %conversation_id,
                sse_receivers = receivers,
                "Runtime evicted; shutdown signal sent, broadcaster preserved for new runtime"
            );
        }
    }

    pub async fn send_event(
        self: &Arc<Self>,
        conversation_id: &str,
        event: Event,
    ) -> Result<(), String> {
        let handle = self.get_or_create(conversation_id).await?;
        handle
            .event_tx
            .send(event)
            .await
            .map_err(|e| format!("Failed to send event: {e}"))
    }

    /// Queue a steering message to be delivered when the conversation next
    /// reaches `Idle`. Persists the entry to DB **before** sending to the
    /// executor channel, so the entry survives a crash between acceptance
    /// and executor processing.
    pub async fn enqueue_steer_message(
        self: &Arc<Self>,
        conversation_id: &str,
        event: Event,
    ) -> Result<(), String> {
        let Event::SteerMessage {
            ref text,
            ref llm_text,
            ref images,
            ref files,
            ref message_id,
            ref user_agent,
            ref skill_invocation,
        } = event
        else {
            return Err("enqueue_steer_message expects Event::SteerMessage".into());
        };

        // Build SteerEntry and persist before touching the executor channel (P1).
        let new_entry = crate::state_machine::event::SteerEntry {
            text: text.clone(),
            llm_text: llm_text.clone(),
            images: images.clone(),
            files: files.clone(),
            message_id: message_id.clone(),
            user_agent: user_agent.clone(),
            skill_invocation: skill_invocation.clone(),
        };
        let db = self.db();
        let mut queue = db
            .get_steering_queue(conversation_id)
            .await
            .map_err(|e| format!("Failed to load steering queue for enqueue: {e}"))?;
        queue.push(new_entry);
        db.update_steering_queue(conversation_id, &queue)
            .await
            .map_err(|e| format!("Failed to persist steering queue before enqueue: {e}"))?;

        // DB is durable; now update the executor's in-memory queue via channel.
        let handle = self.get_or_create(conversation_id).await?;
        handle
            .event_tx
            .send(event)
            .await
            .map_err(|e| format!("Failed to send steer message: {e}"))
    }

    /// Subscribe to conversation updates
    pub async fn subscribe(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<SseEvent>, String> {
        let handle = self.get_or_create(conversation_id).await?;
        Ok(handle.broadcast_tx.subscribe())
    }

    /// Peek at the runtime handle for a conversation without starting one.
    /// Returns `None` if no runtime is currently registered for `conversation_id`.
    ///
    /// REQ-BED-032: the hard-delete cascade uses this to broadcast the
    /// `ConversationHardDeleted` event onto the existing per-conversation
    /// channel — if any. Spinning up a runtime for a conversation that's
    /// about to be deleted would be wasted work (the executor would
    /// observe the row gone seconds later).
    pub async fn try_get_handle(&self, conversation_id: &str) -> Option<ConversationHandle> {
        let runtimes = self.runtimes.read().await;
        runtimes.get(conversation_id).map(|h| ConversationHandle {
            event_tx: h.event_tx.clone(),
            broadcast_tx: h.broadcast_tx.clone(),
            identity: h.identity.clone(),
            state_rx: h.state_rx.clone(),
        })
    }

    /// Return the authoritative `ConvState` for routing decisions.
    ///
    /// If a live runtime exists for `conv_id`, its in-memory executor state is
    /// returned — this is the **only** authority for transient in-flight states
    /// (`LlmRequesting`, `ToolExecuting`, etc.) that have not yet been persisted
    /// to the DB row.  Falls through to `None` when no handle is present;
    /// callers are expected to fall back to the DB row in that case.
    ///
    /// # Authority invariant
    ///
    /// HTTP handlers that make event-routing decisions (e.g. `UserMessage` vs.
    /// `SteerMessage`) **must** call this instead of reading `conversation.state`
    /// directly.  A persisted `Idle` row during restart auto-resume is correct
    /// (it is the safe rest-state before any transition is persisted) but
    /// misleading to a handler that consults only the DB.
    pub async fn effective_conversation_state(&self, conv_id: &str) -> Option<ConvState> {
        let runtimes = self.runtimes.read().await;
        runtimes.get(conv_id).map(|h| h.state_rx.borrow().clone())
    }

    /// Remove and return the evicted broadcaster for `conversation_id`, if any.
    ///
    /// An evicted broadcaster exists in the window between `evict_runtime` (model
    /// upgrade) and the next `get_or_create` call for the same conversation. During
    /// this window the broadcaster is not reachable via `try_get_handle`, so any
    /// caller that needs to push a final event — most notably the hard-delete cascade
    /// — must also check here.
    pub async fn take_evicted_broadcaster(&self, conversation_id: &str) -> Option<SseBroadcaster> {
        // The model-upgrade marker is normally consumed by the next
        // get_or_create. The hard-delete cascade is the one path that takes
        // the evicted broadcaster without a subsequent get_or_create, so the
        // marker would leak forever for a conversation deleted before
        // re-access. Drop it here too — these are the same lifecycle.
        self.evicted_model_upgrades
            .write()
            .await
            .remove(conversation_id);
        self.evicted_broadcasters
            .write()
            .await
            .remove(conversation_id)
    }

    /// Determine the resume state for a conversation.
    ///
    /// Delegates to `recovery::should_auto_continue` for the actual logic.
    /// See that module for comprehensive tests.
    ///
    /// Returns `(state, state_updated_at, needs_auto_continue)`. The
    /// `state_updated_at` is the row's value when the resumed state matches
    /// the persisted row, or `Utc::now()` when auto-continue synthesises a
    /// different state (whose entry time is resume-time). The executor uses
    /// it to seed `ConversationRuntime.state_updated_at` so the first
    /// post-resume `SseEvent::StateChange` carries the real entry time, not
    /// the runtime-construction time (specs/working-phase-visibility/
    /// REQ-WPV-001).
    async fn determine_resume_state(
        &self,
        conversation_id: &str,
    ) -> Result<(ConvState, DateTime<Utc>, bool), String> {
        // States that survive restart (preserved by reset_all_to_idle) must be
        // restored from the DB, not derived from message history. The recovery
        // heuristic only applies to transient states that were reset to Idle.
        let conv = self
            .db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let row_state_updated_at = conv.state_updated_at;

        match &conv.state {
            ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::AwaitingCommissionReviewApproval { .. }
            | ConvState::ContextExhausted { .. }
            | ConvState::HandedOff { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::Terminal => {
                tracing::debug!(
                    conv_id = %conversation_id,
                    state = ?std::mem::discriminant(&conv.state),
                    "Restoring persisted state (survives restart)"
                );
                return Ok((conv.state, row_state_updated_at, false));
            }
            // A usage-limit Error must be restored faithfully so the auto-clear
            // sweep's DismissError lands on an executor that is actually in
            // Error. A model-upgrade eviction (see get_or_create) can drop the
            // live Error executor mid-run; without this, the recreate would
            // derive a non-Error state via the recovery heuristic and silently
            // reject the queued DismissError, leaving the row stuck in
            // UsageLimitReached and the sweep re-firing every tick. (Across a
            // process restart the row is already reset to Idle by
            // reset_all_to_idle, so this arm only matters within one process.)
            ConvState::Error {
                error_kind: crate::db::ErrorKind::UsageLimitReached,
                ..
            } => {
                tracing::debug!(
                    conv_id = %conversation_id,
                    "Restoring persisted usage-limit Error (cleared by the auto-clear sweep)"
                );
                return Ok((conv.state, row_state_updated_at, false));
            }
            _ => {}
        }

        let messages = self
            .db
            .get_messages(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let decision = recovery::should_auto_continue(&messages);

        tracing::debug!(
            conv_id = %conversation_id,
            msg_count = messages.len(),
            reason = ?decision.reason,
            needs_auto_continue = decision.needs_auto_continue,
            "determine_resume_state"
        );

        if decision.needs_auto_continue {
            tracing::info!(
                conv_id = %conversation_id,
                "Detected interrupted conversation - will auto-continue"
            );
        }

        // When auto-continue synthesises a resume state that differs from the
        // persisted (restart-reset) row state, its entry time is *now* — the
        // synthesised phase is entered at resume, not whenever the row was
        // last written. Seeding the row's stamp would make the client's
        // elapsed counter (REQ-WPV-001) run from the prior state's time
        // (potentially hours, for a long-interrupted conversation). When the
        // decision leaves the state unchanged, the row stamp is the true
        // entry time.
        let resume_state_updated_at = if decision.state == conv.state {
            row_state_updated_at
        } else {
            Utc::now()
        };
        Ok((
            decision.state,
            resume_state_updated_at,
            decision.needs_auto_continue,
        ))
    }

    /// Get the database handle
    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn model_registry(&self) -> &ModelRegistry {
        &self.llm_registry
    }

    /// Get the LLM registry
    #[allow(dead_code)] // For future API use
    pub fn llm_registry(&self) -> &Arc<ModelRegistry> {
        &self.llm_registry
    }
}

/// Walk up the parent chain to find the root (top-level) conversation id.
///
/// For a root conversation the function returns immediately. For deeply nested
/// sub-agents it follows `parent_conversation_id` links until it reaches a
/// conversation with no parent, or until the 10-iteration guard fires on
/// corrupt data.
async fn find_root_conversation_id(db: &Database, conversation_id: &str) -> String {
    let mut current_id = conversation_id.to_string();
    for _ in 0..10 {
        match db.get_conversation(&current_id).await {
            Ok(conv) => match conv.parent_conversation_id {
                None => return current_id,
                Some(parent_id) => current_id = parent_id,
            },
            Err(_) => return current_id,
        }
    }
    current_id
}

/// Convert a database `ConvMode` into a `ModeContext` for the system prompt.
pub(crate) fn conv_mode_to_context(mode: &ConvMode) -> ModeContext {
    match mode {
        ConvMode::Explore {
            next_taskmd_id_hint,
            ..
        } => ModeContext::Explore {
            next_taskmd_id_hint: next_taskmd_id_hint.as_ref().map(ToString::to_string),
        },
        ConvMode::Work {
            branch_name,
            base_branch,
            worktree_path,
            ..
        } => ModeContext::Work {
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            worktree_path: worktree_path.to_string(),
        },
        ConvMode::Branch {
            branch_name,
            base_branch,
            worktree_path,
        } => ModeContext::Branch {
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            worktree_path: worktree_path.to_string(),
        },
        ConvMode::Direct => ModeContext::Direct,
    }
}

#[cfg(test)]
mod sub_agent_registry_resume_tests {
    //! Regression coverage for the resume-path tool-registry selection
    //! (`runtime.rs` ~1267, subagents.allium `SubAgentRegistryOnResume`).
    //!
    //! Before task 13010 this branch hard-coded `for_subagent_explore()`,
    //! silently stripping `patch` from a re-created Work sub-agent. The
    //! `sub_agent_registry_for_conv_mode` helper now picks the right
    //! registry from the persisted `conv_mode`; these tests pin that
    //! contract so a future refactor cannot quietly regress it.
    use super::sub_agent_registry_for_conv_mode;
    use crate::db::{ConvMode, NonEmptyString};
    use crate::platform::PlatformCapability;
    use crate::tools::ExploreToolPolicy;

    fn registry_has(conv_mode: &ConvMode, tool: &str) -> bool {
        sub_agent_registry_for_conv_mode(
            conv_mode,
            ExploreToolPolicy::from_platform(&PlatformCapability::None {
                details: "test".into(),
            }),
        )
        .definitions()
        .iter()
        .any(|d| d.name == tool)
    }

    #[test]
    fn explore_subagent_resume_excludes_patch() {
        let mode = ConvMode::Explore {
            worktree_path: None,
            next_taskmd_id_hint: None,
        };
        assert!(!registry_has(&mode, "patch"));
        assert!(registry_has(&mode, "submit_result"));
    }

    #[test]
    fn direct_subagent_resume_keeps_patch() {
        // A Work sub-agent spawned from a Direct parent persists as
        // ConvMode::Direct (Work sub-agents inherit parent conv_mode).
        // The previous bug mapped Direct -> Explore registry; the
        // resumed sub-agent must keep `patch`.
        assert!(registry_has(&ConvMode::Direct, "patch"));
    }

    #[test]
    fn work_subagent_resume_keeps_patch() {
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("task-0001-x").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("0001").unwrap(),
            task_title: NonEmptyString::new("x").unwrap(),
        };
        assert!(registry_has(&mode, "patch"));
    }

    #[test]
    fn branch_subagent_resume_keeps_patch() {
        let mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("feature-x").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt").unwrap(),
            base_branch: NonEmptyString::new("feature-x").unwrap(),
        };
        assert!(registry_has(&mode, "patch"));
    }
}

#[cfg(test)]
mod work_scope_derivation_tests {
    //! The scope keying used to address a conversation's bash / browser /
    //! tmux handles (`ToolContext.work_scope`) MUST agree with the scope the
    //! DB-facing paths derive (inventory assembler, hard-delete cleanup
    //! cascade, work-scope SSE routing, browser-liveness reap). Both sides
    //! resolve from a single authority: the persisted
    //! `ConvMode::worktree_path()`.
    //!
    //! Regression: an Explore sub-agent persists as
    //! `ConvMode::Explore { worktree_path: None }`, so its DB-facing scope is
    //! `WorkScope::Conversation(id)` (REQ-BASH-WS-001: "Direct-mode
    //! conversations and sub-agents resolve to `WorkScope::Conversation(id)`").
    //! A prior tool-side derivation keyed off `mode != Direct → working_dir`,
    //! which gave a managed sub-agent `WorkScope::Worktree(cwd)` instead —
    //! diverging from every DB-facing path, so its handles never appeared in
    //! its own inventory and deleting the parent's worktree scope could kill
    //! the live sub-agent's handles.
    use crate::db::{ConvMode, NonEmptyString};
    use crate::work_scope::WorkScope;
    use std::path::Path;

    /// The DB-facing derivation, mirrored from `WorkScope::resolve(conv.id,
    /// conv.conv_mode.worktree_path())` as used by the inventory / cleanup /
    /// SSE paths in this module.
    fn db_facing_scope(conv_id: &str, conv_mode: &ConvMode) -> WorkScope {
        WorkScope::resolve(conv_id, conv_mode.worktree_path().map(Path::new))
    }

    /// The tool-side derivation. `ConvContext.work_scope_worktree` is set from
    /// `conv_mode.worktree_path()` at both runtime construction sites (spawn +
    /// resume), and the executor passes it to `ToolContext::new`, which calls
    /// `WorkScope::resolve(conv_id, work_scope_worktree)`. We replicate that
    /// chain here without standing up a full runtime.
    fn tool_side_scope(conv_id: &str, conv_mode: &ConvMode) -> WorkScope {
        let work_scope_worktree = conv_mode.worktree_path().map(std::path::PathBuf::from);
        WorkScope::resolve(conv_id, work_scope_worktree.as_deref())
    }

    fn assert_scopes_agree(conv_id: &str, conv_mode: &ConvMode, expected: &WorkScope) {
        let db = db_facing_scope(conv_id, conv_mode);
        let tool = tool_side_scope(conv_id, conv_mode);
        assert_eq!(db, tool, "tool-side and DB-facing scope must agree");
        assert_eq!(&tool, expected);
    }

    #[test]
    fn explore_subagent_resolves_to_its_own_conversation_scope() {
        let mode = ConvMode::Explore {
            worktree_path: None,
            next_taskmd_id_hint: None,
        };
        assert_scopes_agree(
            "explore-subagent-1",
            &mode,
            &WorkScope::Conversation("explore-subagent-1".to_string()),
        );
    }

    #[test]
    fn top_level_explore_resolves_to_its_worktree_scope() {
        let mode = ConvMode::Explore {
            worktree_path: Some(NonEmptyString::new("/tmp/wt-explore").unwrap()),
            next_taskmd_id_hint: None,
        };
        assert_scopes_agree(
            "explore-toplevel-1",
            &mode,
            &WorkScope::Worktree("/tmp/wt-explore".to_string()),
        );
    }

    #[test]
    fn work_subagent_shares_parent_worktree_scope() {
        // A Work sub-agent inherits the parent's conv_mode (worktree included)
        // and so co-owns the parent's worktree scope (REQ-BASH-WS-002).
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("task-0001-x").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt-work").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("0001").unwrap(),
            task_title: NonEmptyString::new("x").unwrap(),
        };
        assert_scopes_agree(
            "work-subagent-1",
            &mode,
            &WorkScope::Worktree("/tmp/wt-work".to_string()),
        );
    }

    #[test]
    fn branch_resolves_to_its_worktree_scope() {
        let mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("feature-x").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt-branch").unwrap(),
            base_branch: NonEmptyString::new("feature-x").unwrap(),
        };
        assert_scopes_agree(
            "branch-1",
            &mode,
            &WorkScope::Worktree("/tmp/wt-branch".to_string()),
        );
    }

    #[test]
    fn direct_resolves_to_its_conversation_scope() {
        assert_scopes_agree(
            "direct-1",
            &ConvMode::Direct,
            &WorkScope::Conversation("direct-1".to_string()),
        );
    }
}

#[cfg(test)]
mod broadcaster_tests {
    use super::*;

    /// Regression for task 02679: when a caller pre-allocates a message's
    /// `sequence_id` from the broadcaster *before* writing to the DB, the
    /// message's seq is strictly greater than any ephemeral event emitted
    /// earlier on the same broadcaster. A concurrent reader (the client's
    /// `applyIfNewer` guard) will accept the message rather than dropping
    /// it as stale.
    ///
    /// The failure shape this guards against:
    /// - pre-fix, `add_message` allocated its own seq via `SELECT MAX+1`.
    /// - After several ephemeral events (tokens advance `SseBroadcaster`'s
    ///   counter to N ≫ DB message count), an assistant message persists
    ///   with DB seq = (count+1) ≪ N.
    /// - `send_message` broadcasts it with that stale seq; the client's
    ///   `lastSequenceId ≥ N` causes `applyIfNewer` to drop it; the
    ///   assistant's response visibly disappears.
    ///
    /// See `specs/sse_wire/sse_wire.allium`, invariant `PersistBeforeBroadcast`.
    #[test]
    fn next_seq_after_ephemeral_events_exceeds_prior_events() {
        let b = SseBroadcaster::new(16, 0);

        // Simulate many ephemeral events (token stream, state changes)
        // each consuming one seq from the counter.
        let mut last_ephemeral = 0;
        for _ in 0..50 {
            last_ephemeral = b.next_seq();
        }

        // Pre-allocate a seq for a message that's about to be persisted.
        let message_seq = b.next_seq();

        // The message seq must be strictly greater than every ephemeral
        // event emitted before it. This is the structural property the
        // client's applyIfNewer guard relies on.
        assert!(
            message_seq > last_ephemeral,
            "pre-allocated message seq ({message_seq}) must exceed all prior \
             ephemeral seqs ({last_ephemeral})"
        );
        assert_eq!(message_seq, last_ephemeral + 1);
    }

    /// `observe_seq` is idempotent when the broadcaster's counter is
    /// already past the supplied seq — this is the normal path once
    /// `send_message` runs with a pre-allocated seq (broadcaster counter
    /// already = seq after `next_seq()`).
    #[test]
    fn observe_seq_is_idempotent_when_counter_already_past() {
        let b = SseBroadcaster::new(16, 0);
        let seq = b.next_seq();
        b.observe_seq(seq);
        assert_eq!(
            b.current_seq(),
            seq,
            "observe_seq must not bump the counter past seq when already at seq"
        );

        // A subsequent next_seq advances by exactly one.
        let next = b.next_seq();
        assert_eq!(next, seq + 1);
    }

    /// `observe_seq` still catches up when a DB-allocated message seq
    /// leapfrogs the broadcaster — the pre-fix path. Kept as a belt-and-
    /// braces check: non-broadcasting paths (sub-agent bootstrap, crash
    /// recovery) still use `add_message`, and the first `send_message` on
    /// a restarted conversation must fold their DB seqs back in.
    #[test]
    fn observe_seq_catches_up_when_db_seq_leapfrogs() {
        let b = SseBroadcaster::new(16, 0);
        // Simulate: two direct-DB writes (bootstrap + restart marker)
        // occurred before the broadcaster emitted anything; broadcaster
        // is at 0, DB MAX is 2.
        b.observe_seq(2);
        let next = b.next_seq();
        assert_eq!(next, 3, "broadcaster must allocate past the DB watermark");
    }

    // ── ReplayRing ──────────────────────────────────────────────────────

    /// Make a test `Token` event with the given seq. Token is the
    /// dominant ring entry in practice; using it keeps the byte-size
    /// estimate small and the assertions readable.
    fn token_event(seq: i64, text: &str) -> SseEvent {
        SseEvent::Token {
            sequence_id: seq,
            text: text.to_string(),
            request_id: "test-req".to_string(),
        }
    }

    /// Build a minimal `crate::db::Message` for ring tests. Persisted vs
    /// ephemeral selection happens at the broadcaster API layer, not via
    /// any field on the struct itself.
    fn test_message(seq: i64, message_id: &str) -> crate::db::Message {
        use crate::db::{MessageContent, MessageType};
        use chrono::Utc;
        crate::db::Message {
            message_id: message_id.to_string(),
            conversation_id: "test-conv".to_string(),
            sequence_id: seq,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![phoenix_llm::ContentBlock::text("hi")]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    /// Fresh broadcaster: ring is empty, anchor matches `initial_last_seq`,
    /// truncated is false.
    #[test]
    fn replay_ring_starts_empty_and_anchored_at_initial_seq() {
        let b = SseBroadcaster::new(16, 5);
        let (anchor, truncated, highest, events) = b.snapshot_pending();
        assert_eq!(anchor, 5, "anchor should match initial_last_seq");
        assert!(!truncated);
        assert_eq!(
            highest, 5,
            "empty ring reports highest_seq equal to the anchor"
        );
        assert!(events.is_empty());
        assert_eq!(b.replay_ring_bytes(), 0);
    }

    /// `send_seq` (ephemeral) appends the event to the ring.
    #[test]
    fn send_seq_appends_to_replay_ring() {
        let b = SseBroadcaster::new(16, 0);
        // A subscriber is required for the channel send to succeed; otherwise
        // `tx.send` would return SendError. The ring path runs first, so the
        // append happens even with no subscriber, but for parity with real
        // usage we attach one.
        let _rx = b.subscribe();

        let _ = b.send_seq(|seq| token_event(seq, "hello"));
        let _ = b.send_seq(|seq| token_event(seq, "world"));

        let (anchor, truncated, highest, events) = b.snapshot_pending();
        assert_eq!(anchor, 0, "no persisted Message yet; anchor stays at 0");
        assert!(!truncated);
        assert_eq!(
            highest, 2,
            "highest_seq tracks the last entry's seq when the ring is populated"
        );
        assert_eq!(events.len(), 2);
        assert!(b.replay_ring_bytes() > 0);

        // Replayed events carry their original seqs.
        match &events[0] {
            SseEvent::Token {
                sequence_id, text, ..
            } => {
                assert_eq!(*sequence_id, 1);
                assert_eq!(text, "hello");
            }
            other => panic!("expected Token, got {other:?}"),
        }
        match &events[1] {
            SseEvent::Token { sequence_id, .. } => assert_eq!(*sequence_id, 2),
            other => panic!("expected Token, got {other:?}"),
        }
    }

    /// `send_persisted_message` resets the ring: anchor advances, entries
    /// clear, truncated flag clears, byte counter resets.
    #[test]
    fn send_persisted_message_resets_ring_anchor() {
        let b = SseBroadcaster::new(16, 0);
        let _rx = b.subscribe();

        // Two ephemeral events sit in the ring.
        let _ = b.send_seq(|seq| token_event(seq, "a"));
        let _ = b.send_seq(|seq| token_event(seq, "b"));

        // Persisted message arrives.
        let msg = test_message(3, "msg-1");
        let _ = b.send_persisted_message(msg);

        let (anchor, truncated, highest, events) = b.snapshot_pending();
        assert_eq!(anchor, 3, "anchor advances to the persisted message's seq");
        assert!(!truncated);
        assert_eq!(
            highest, 3,
            "empty post-reset ring reports highest_seq = anchor"
        );
        assert!(events.is_empty(), "ring entries cleared on anchor reset");
        assert_eq!(b.replay_ring_bytes(), 0);
    }

    /// `send_ephemeral_message` appends a `Message` event to the ring
    /// without resetting the anchor — the eager-broadcast path.
    #[test]
    fn send_ephemeral_message_appends_without_reset() {
        let b = SseBroadcaster::new(16, 0);
        let _rx = b.subscribe();

        // First, a real anchor.
        let _ = b.send_persisted_message(test_message(1, "anchor"));
        // Then an ephemeral state_change.
        let _ = b.send_seq(|seq| SseEvent::Token {
            sequence_id: seq,
            text: "x".to_string(),
            request_id: "r".to_string(),
        });
        // Then the eager assistant message.
        let _ = b.send_ephemeral_message(test_message(3, "eager"));

        let (anchor, truncated, highest, events) = b.snapshot_pending();
        assert_eq!(anchor, 1, "eager message does not advance the anchor");
        assert!(!truncated);
        assert_eq!(highest, 3, "highest_seq covers the eager message at seq 3");
        assert_eq!(events.len(), 2, "token + eager message both in ring");
        match &events[1] {
            SseEvent::Message { message } => assert_eq!(message.message_id, "eager"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Overflow: appending past `REPLAY_RING_CAPACITY` triggers
    /// clear-and-truncate; subsequent appends are no-ops; snapshot returns
    /// `truncated=true` and empty events (force full resync, Q3 resolution).
    #[test]
    fn replay_ring_overflow_clears_and_truncates() {
        let b = SseBroadcaster::new(REPLAY_RING_CAPACITY * 2, 0);
        let _rx = b.subscribe();

        // Fill the ring exactly to capacity.
        for i in 0..REPLAY_RING_CAPACITY {
            let _ = b.send_seq(|seq| token_event(seq, &format!("t{i}")));
        }
        {
            let (_, truncated, _, events) = b.snapshot_pending();
            assert!(!truncated, "exactly-at-capacity is not yet truncated");
            assert_eq!(events.len(), REPLAY_RING_CAPACITY);
        }

        // One more pushes past the cap — clear and truncate.
        let _ = b.send_seq(|seq| token_event(seq, "overflow"));
        let (anchor, truncated, highest, events) = b.snapshot_pending();
        assert_eq!(anchor, 0, "anchor unchanged across overflow");
        assert!(truncated, "ring should be marked truncated");
        assert_eq!(
            highest, 0,
            "truncated ring falls back to anchor for highest_seq"
        );
        assert!(
            events.is_empty(),
            "snapshot returns empty events on truncation"
        );
        assert_eq!(b.replay_ring_bytes(), 0);

        // Further appends are no-ops within this anchor window.
        let _ = b.send_seq(|seq| token_event(seq, "after-truncate"));
        let (_, truncated2, _, events2) = b.snapshot_pending();
        assert!(truncated2);
        assert!(events2.is_empty());
    }

    /// Persisted Message after truncation clears the truncated flag.
    #[test]
    fn persisted_message_clears_truncated_flag() {
        let b = SseBroadcaster::new(REPLAY_RING_CAPACITY * 2, 0);
        let _rx = b.subscribe();

        // Overflow.
        for i in 0..=REPLAY_RING_CAPACITY {
            let _ = b.send_seq(|seq| token_event(seq, &format!("t{i}")));
        }
        assert!(b.snapshot_pending().1, "should be truncated");

        // Persisted message resets.
        let next_seq = b.next_seq();
        let _ = b.send_persisted_message(test_message(next_seq, "msg"));

        let (anchor, truncated, _, events) = b.snapshot_pending();
        assert_eq!(anchor, next_seq);
        assert!(!truncated, "anchor reset clears truncated flag");
        assert!(events.is_empty());

        // Future appends accumulate normally.
        let _ = b.send_seq(|seq| token_event(seq, "post-reset"));
        let (_, _, _, events) = b.snapshot_pending();
        assert_eq!(events.len(), 1);
    }

    /// Ring entries appear in strictly increasing `sequence_id` order
    /// (preserved by FIFO append + anchor-reset clear).
    #[test]
    fn replay_ring_entries_in_seq_order() {
        let b = SseBroadcaster::new(64, 10);
        let _rx = b.subscribe();
        for i in 0..20 {
            let _ = b.send_seq(|seq| token_event(seq, &format!("t{i}")));
        }
        let (_, _, _, events) = b.snapshot_pending();
        let seqs: Vec<i64> = events
            .iter()
            .map(|e| match e {
                SseEvent::Token { sequence_id, .. } => *sequence_id,
                _ => unreachable!(),
            })
            .collect();
        for w in seqs.windows(2) {
            assert!(
                w[0] < w[1],
                "entries must be in strictly increasing seq order"
            );
        }
    }

    /// Snapshot returns entries sorted by `sequence_id` even when
    /// appends arrive out of seq order. Models the
    /// `next_seq` → build → ring-mutex race where two tasks allocate
    /// in one order and lock the ring in the opposite order.
    #[test]
    fn replay_ring_snapshot_sorts_out_of_order_appends() {
        let mut ring = ReplayRing::new();
        // Two tasks: A allocated seq 5 first, B allocated 6 second.
        // B raced ahead and appended first.
        ring.append(ReplayRingEntry {
            event: token_event(6, "b"),
            sequence_id: 6,
        });
        ring.append(ReplayRingEntry {
            event: token_event(5, "a"),
            sequence_id: 5,
        });

        let (_, _, _, events) = ring.snapshot();
        assert_eq!(events.len(), 2);
        let seqs: Vec<i64> = events
            .iter()
            .map(|e| {
                let SseEvent::Token { sequence_id, .. } = e else {
                    unreachable!()
                };
                *sequence_id
            })
            .collect();
        assert_eq!(
            seqs,
            vec![5, 6],
            "snapshot must sort entries by sequence_id"
        );
    }

    /// An append with `sequence_id <= anchor_seq` is dropped. Models the
    /// race where a persisted-Message broadcast advances the anchor
    /// between a sender's `next_seq` and the sender's ring-mutex acquire.
    #[test]
    fn replay_ring_append_below_anchor_dropped() {
        let mut ring = ReplayRing::new();
        ring.anchor_seq = 10;

        // Late append with seq <= anchor: dropped.
        ring.append(ReplayRingEntry {
            event: token_event(8, "stale"),
            sequence_id: 8,
        });
        ring.append(ReplayRingEntry {
            event: token_event(10, "boundary"),
            sequence_id: 10,
        });
        // Above anchor: accepted.
        ring.append(ReplayRingEntry {
            event: token_event(11, "fresh"),
            sequence_id: 11,
        });

        let (anchor, _, _, events) = ring.snapshot();
        assert_eq!(anchor, 10);
        assert_eq!(events.len(), 1, "only seq=11 should remain");
        let SseEvent::Token { sequence_id, .. } = &events[0] else {
            unreachable!()
        };
        assert_eq!(*sequence_id, 11);
    }

    /// Every ring entry has seq strictly greater than the ring's anchor.
    #[test]
    fn replay_ring_entries_above_anchor() {
        let b = SseBroadcaster::new(64, 0);
        let _rx = b.subscribe();

        let _ = b.send_persisted_message(test_message(5, "anchor"));
        for i in 0..5 {
            let _ = b.send_seq(|seq| token_event(seq, &format!("t{i}")));
        }

        let (anchor, _, _, events) = b.snapshot_pending();
        assert_eq!(anchor, 5);
        for e in events {
            let SseEvent::Token {
                sequence_id: seq, ..
            } = e
            else {
                unreachable!()
            };
            assert!(seq > anchor, "every entry's seq must exceed anchor");
        }
    }

    /// `snapshot_pending` reports a `highest_seq` that bounds every entry
    /// in the snapshot. Regression test for the race PR #76 review caught:
    /// reading `current_seq` separately would let a sender's mid-flight
    /// allocation produce a `last_sequence_id` that exceeds what the
    /// snapshot actually covers (or, conversely, leave a snapshot entry
    /// with seq above `last_sequence_id`, violating
    /// `sse_wire.allium` `StreamOpened`).
    #[test]
    fn snapshot_pending_highest_seq_bounds_entries() {
        let b = SseBroadcaster::new(64, 10);
        let _rx = b.subscribe();
        for i in 0..7 {
            let _ = b.send_seq(|seq| token_event(seq, &format!("t{i}")));
        }
        let (anchor, _, highest, events) = b.snapshot_pending();
        for e in &events {
            let SseEvent::Token {
                sequence_id: seq, ..
            } = e
            else {
                unreachable!()
            };
            assert!(*seq <= highest, "entry seq must not exceed highest_seq");
            assert!(*seq > anchor, "entry seq must exceed anchor");
        }
        assert_eq!(
            highest,
            anchor + i64::try_from(events.len()).expect("ring length fits i64"),
            "with seq 11..17 in the ring, highest is 17"
        );
    }
}

#[cfg(test)]
mod scope_liveness_tests {
    //! `scope_has_live_conversation[_excluding]` derives liveness from the
    //! DATABASE, not the live-runtime-handle set. Two properties matter:
    //!
    //! - A non-terminal, non-archived sibling that resolves to the scope
    //!   preserves it even when it has NO runtime handle (post-restart /
    //!   post-eviction). Counting only handles would let the cascade tear
    //!   down a worktree/branch and bash/tmux/browser still owned by a live
    //!   conversation — data loss.
    //! - An archived conversation is not a live owner even when its DB row
    //!   still reads non-terminal: archiving a Work/Branch chain archives
    //!   earlier members before the leaf's cleanup cascade runs; counting an
    //!   archived member as live would preserve the shared `WorkScope` and
    //!   leak its bash/tmux/browser/terminal resources.
    use super::*;
    use crate::platform::PlatformCapability;
    use crate::tools::mcp::McpClientManager;
    use phoenix_core::domain::db_schema::{ConvMode, NonEmptyString};
    use phoenix_core::domain::sm_state::ConvState;
    use phoenix_llm::ModelRegistry;

    fn work_mode(worktree_path: &str) -> ConvMode {
        ConvMode::Work {
            branch_name: NonEmptyString::new("task-branch").unwrap(),
            worktree_path: NonEmptyString::new(worktree_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("T1").unwrap(),
            task_title: NonEmptyString::new("title").unwrap(),
        }
    }

    /// Create a non-user-initiated conversation in Work mode on `worktree_path`
    /// and give it NO runtime handle — exactly the post-restart / post-eviction
    /// shape the regression guards against.
    async fn create_handleless_work_conv(mgr: &RuntimeManager, id: &str, worktree_path: &str) {
        mgr.db()
            .create_conversation_with_project(
                id,
                id,
                worktree_path,
                false,
                None,
                None,
                None,
                &work_mode(worktree_path),
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .expect("create work conv");
    }

    async fn test_manager() -> RuntimeManager {
        let db = crate::db::Database::open_in_memory().await.expect("db");
        RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(McpClientManager::new()),
            None,
        )
    }

    /// Register a lingering runtime handle for `conv_id` without spawning a
    /// real conversation runtime — mirrors the handle the executor inserts.
    async fn register_lingering_handle(mgr: &RuntimeManager, conv_id: &str) {
        let (event_tx, _event_rx) = mpsc::channel(1);
        let (_state_tx, state_rx) = watch::channel(ConvState::Idle);
        mgr.runtimes.write().await.insert(
            conv_id.to_string(),
            ConversationHandle {
                event_tx,
                broadcast_tx: SseBroadcaster::new(SSE_BROADCAST_CAPACITY, 0),
                identity: Arc::new(()),
                state_rx,
            },
        );
    }

    #[tokio::test]
    async fn non_terminal_unarchived_conversation_counts_as_live() {
        let mgr = test_manager().await;
        mgr.db()
            .create_conversation("conv-live", "slug", "/tmp", true, None, None)
            .await
            .expect("create");
        register_lingering_handle(&mgr, "conv-live").await;

        let scope = crate::work_scope::WorkScope::Conversation("conv-live".to_string());
        assert!(
            mgr.scope_has_live_conversation(&scope).await.unwrap(),
            "a non-terminal, unarchived owner with a live handle is live"
        );
    }

    #[tokio::test]
    async fn determine_resume_state_preserves_usage_limit_error() {
        // An executor recreated mid-run (e.g. model-upgrade eviction) for a
        // usage-limit-errored conversation must resume in Error, so the
        // auto-clear sweep's DismissError applies instead of being silently
        // rejected against a derived non-Error state.
        let mgr = test_manager().await;
        mgr.db()
            .create_conversation("ul", "slug", "/tmp", true, None, None)
            .await
            .expect("create");
        let reset = Utc::now();
        mgr.db()
            .update_conversation_state(
                "ul",
                &ConvState::Error {
                    message: "You've hit your usage limit.".into(),
                    error_kind: crate::db::ErrorKind::UsageLimitReached,
                    resets_at: Some(reset),
                },
            )
            .await
            .expect("set error");

        let (state, _ts, needs_auto_continue) =
            mgr.determine_resume_state("ul").await.expect("resume");

        assert!(
            matches!(
                state,
                ConvState::Error {
                    error_kind: crate::db::ErrorKind::UsageLimitReached,
                    resets_at: Some(_),
                    ..
                }
            ),
            "usage-limit Error must be restored on recreate, got {state:?}"
        );
        assert!(!needs_auto_continue);
    }

    #[tokio::test]
    async fn determine_resume_state_does_not_preserve_other_errors() {
        // Only usage-limit Error is preserved; other error kinds keep the
        // recovery-heuristic path (here, no messages -> derived to non-Error).
        let mgr = test_manager().await;
        mgr.db()
            .create_conversation("net", "slug", "/tmp", true, None, None)
            .await
            .expect("create");
        mgr.db()
            .update_conversation_state(
                "net",
                &ConvState::Error {
                    message: "network".into(),
                    error_kind: crate::db::ErrorKind::Network,
                    resets_at: None,
                },
            )
            .await
            .expect("set error");

        let (state, _ts, _needs) = mgr.determine_resume_state("net").await.expect("resume");

        assert!(
            !matches!(state, ConvState::Error { .. }),
            "a non-usage-limit Error must not be preserved, got {state:?}"
        );
    }

    #[tokio::test]
    async fn archived_conversation_does_not_count_as_live() {
        let mgr = test_manager().await;
        mgr.db()
            .create_conversation("conv-arch", "slug", "/tmp", true, None, None)
            .await
            .expect("create");
        register_lingering_handle(&mgr, "conv-arch").await;

        // Sanity: live before archiving.
        let scope = crate::work_scope::WorkScope::Conversation("conv-arch".to_string());
        assert!(mgr.scope_has_live_conversation(&scope).await.unwrap());

        // Archive the (still non-terminal) conversation; the lingering
        // runtime handle stays registered, as it does in the real cascade.
        mgr.db()
            .archive_conversation("conv-arch")
            .await
            .expect("archive");
        let conv = mgr.db().get_conversation("conv-arch").await.expect("get");
        assert!(conv.archived, "precondition: archived flag set");
        assert!(
            !conv.state.is_terminal(),
            "precondition: row still reads non-terminal"
        );

        assert!(
            !mgr.scope_has_live_conversation(&scope).await.unwrap(),
            "an archived conversation must not preserve its scope"
        );
    }

    /// Regression: a non-terminal, non-archived sibling with NO runtime handle
    /// still preserves its shared worktree scope. Deleting the leaf
    /// (`excluded_conv_id`) must NOT let the cascade tear down the worktree /
    /// branch / bash because the parent — handle-less after a restart — still
    /// resolves to the same `WorkScope`.
    #[tokio::test]
    async fn handleless_non_terminal_sibling_preserves_worktree_scope() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        // Parent (surviving owner) — non-terminal, non-archived, NO handle.
        create_handleless_work_conv(&mgr, "parent", worktree).await;
        // Leaf sub-agent being deleted — also on the shared worktree.
        create_handleless_work_conv(&mgr, "leaf", worktree).await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "handle-less non-terminal parent still owns the shared worktree scope"
        );
    }

    /// Counterpart: when the deleted leaf is genuinely the last live owner
    /// (the sibling has gone terminal), the scope is NOT preserved and the
    /// cascade tears down.
    #[tokio::test]
    async fn truly_last_owner_does_not_preserve_worktree_scope() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        create_handleless_work_conv(&mgr, "parent", worktree).await;
        create_handleless_work_conv(&mgr, "leaf", worktree).await;

        // Parent reaches a terminal state — only the leaf remains live.
        mgr.db()
            .update_conversation_state(
                "parent",
                &ConvState::Completed {
                    result: "done".to_string(),
                },
            )
            .await
            .expect("terminate parent");

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            !mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "with the only other owner terminal, deleting the leaf leaves no live owner"
        );
    }

    /// Regression (REQ-BED-031): a `ContextExhausted` conversation owns its
    /// preserved worktree pending the user's `Continue` / `Abandon` / `MarkAsMerged`
    /// decision. Deleting a SIBLING sub-agent on the same worktree must NOT let
    /// the cleanup cascade conclude the scope is unowned and force-remove the
    /// worktree — that destroys uncommitted user work.
    #[tokio::test]
    async fn context_exhausted_sibling_preserves_worktree_scope() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        // The parent that hit ContextExhausted — owns the preserved worktree.
        create_handleless_work_conv(&mgr, "parent", worktree).await;
        mgr.db()
            .update_conversation_state(
                "parent",
                &ConvState::ContextExhausted {
                    summary: "ran out of context".to_string(),
                },
            )
            .await
            .expect("set context-exhausted");

        // Sibling sub-agent on the shared worktree being deleted.
        create_handleless_work_conv(&mgr, "leaf", worktree).await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "a ContextExhausted parent still owns the shared worktree — deleting a \
             sibling must not tear it down"
        );
    }

    /// A `ContextExhausted` parent that HAS been continued does NOT own the
    /// shared worktree — ownership has transferred to the live continuation
    /// (the leaf). When that leaf is itself being cleaned up (it is the
    /// `excluded_conv_id`), the continued parent must NOT count as an owner,
    /// or the leaf's worktree would be wrongly preserved and never torn down.
    /// This unifies continued `ContextExhausted` with the `HandedOff`
    /// chain-liveness rule.
    #[tokio::test]
    async fn continued_context_exhausted_parent_does_not_block_leaf_cleanup() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        // Parent hit ContextExhausted, then the user chose Continue — the
        // continuation (leaf) now owns the worktree.
        create_handleless_work_conv(&mgr, "parent", worktree).await;
        create_handleless_work_conv(&mgr, "leaf", worktree).await;
        mgr.db()
            .update_conversation_state(
                "parent",
                &ConvState::ContextExhausted {
                    summary: "ran out of context".to_string(),
                },
            )
            .await
            .expect("set context-exhausted");
        // Wire the continuation edge the chain walk reads.
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind("leaf")
            .bind("parent")
            .execute(mgr.db().pool())
            .await
            .expect("wire continuation");
        // Refresh the in-memory row so `continued_in_conv_id` is populated.
        let parent = mgr.db().get_conversation("parent").await.expect("get");
        assert_eq!(
            parent.continued_in_conv_id.as_deref(),
            Some("leaf"),
            "precondition: parent is continued by the leaf"
        );

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        // The leaf (the live continuation) is being abandoned/cleaned: it is
        // the excluded_conv_id. With nothing else live on the chain, the
        // continued ContextExhausted parent must NOT own the scope, so the
        // leaf's worktree can be removed.
        assert!(
            !mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "a CONTINUED ContextExhausted parent does not own the scope while its \
             continuation (the leaf) is being cleaned up"
        );

        // Counterpart: while the live continuation (leaf) is NOT the one being
        // cleaned (an unrelated sibling is), the live leaf still owns the
        // shared scope — the continued parent transferred ownership to it, so
        // the worktree stays preserved.
        create_handleless_work_conv(&mgr, "sibling", worktree).await;
        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "sibling")
                .await
                .unwrap(),
            "the live continuation (leaf) owns the scope; deleting an unrelated \
             sibling must not tear it down"
        );
    }

    /// A `HandedOff` row whose continuation has gone terminal (the whole
    /// forward chain dead-ends) is the last protector of the preserved
    /// worktree. Deleting a sibling must not tear it down.
    #[tokio::test]
    async fn handed_off_dead_end_chain_preserves_worktree_scope() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        // Parent handed off to a successor that itself reached a terminal
        // state — the chain has no live owner downstream.
        create_handleless_work_conv(&mgr, "parent", worktree).await;
        create_handleless_work_conv(&mgr, "successor", worktree).await;
        mgr.db()
            .update_conversation_state(
                "parent",
                &ConvState::HandedOff {
                    successor_conv_id: "successor".to_string(),
                },
            )
            .await
            .expect("set handed-off");
        mgr.db()
            .update_conversation_state("successor", &ConvState::Terminal)
            .await
            .expect("terminate successor");
        // Wire the continuation edge the chain walk reads.
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind("successor")
            .bind("parent")
            .execute(mgr.db().pool())
            .await
            .expect("wire continuation");

        // Sibling sub-agent on the shared worktree being deleted.
        create_handleless_work_conv(&mgr, "leaf", worktree).await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "a HandedOff row whose continuation chain is dead still protects the \
             preserved worktree"
        );
    }

    /// Counterpart: a `HandedOff` row WITH a live continuation does NOT itself
    /// own the scope — but the live continuation does, so the scope is still
    /// preserved. (The point of this test is that ownership is attributed to
    /// the continuation, matching `reconcile_worktrees` skipping the parent.)
    #[tokio::test]
    async fn handed_off_with_live_continuation_scope_owned_by_continuation() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        create_handleless_work_conv(&mgr, "parent", worktree).await;
        create_handleless_work_conv(&mgr, "successor", worktree).await;
        mgr.db()
            .update_conversation_state(
                "parent",
                &ConvState::HandedOff {
                    successor_conv_id: "successor".to_string(),
                },
            )
            .await
            .expect("set handed-off");
        // successor stays non-terminal (live).
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind("successor")
            .bind("parent")
            .execute(mgr.db().pool())
            .await
            .expect("wire continuation");

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        // Deleting the live successor leaf: the only remaining row is the
        // HandedOff parent, whose continuation (the leaf) is being deleted.
        // With nothing live downstream, the parent now protects the worktree.
        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "successor")
                .await
                .unwrap(),
            "deleting the live continuation leaves the HandedOff parent as the owner"
        );

        // Deleting an unrelated leaf while the successor is still live: the
        // successor owns the scope.
        create_handleless_work_conv(&mgr, "leaf", worktree).await;
        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "the live continuation owns the scope"
        );
    }

    /// Helper: wire a `continued_in_conv_id` edge `from` → `to`.
    async fn wire_continuation(mgr: &RuntimeManager, from: &str, to: &str) {
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(to)
            .bind(from)
            .execute(mgr.db().pool())
            .await
            .expect("wire continuation");
    }

    async fn set_context_exhausted(mgr: &RuntimeManager, id: &str) {
        mgr.db()
            .update_conversation_state(
                id,
                &ConvState::ContextExhausted {
                    summary: "ran out of context".to_string(),
                },
            )
            .await
            .expect("set context-exhausted");
    }

    /// Multi-hop regression: A→B→C where A and B are BOTH
    /// `ContextExhausted`-and-continued and C (the live leaf) is the row being
    /// cleaned up. An intermediate continued `ContextExhausted` (B) must NOT
    /// count as a live owner merely for being `ContextExhausted` — it ceded to
    /// its own downstream chain (C), which is excluded. With nothing live left,
    /// the scope is NOT owned, so the preserved worktree is removable. Before
    /// the fix B (then A) kept the worktree alive, leaking it after the live
    /// leaf was gone.
    #[tokio::test]
    async fn multi_hop_continued_context_exhausted_chain_does_not_block_leaf_cleanup() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        // A → B → C, all on the shared worktree.
        create_handleless_work_conv(&mgr, "A", worktree).await;
        create_handleless_work_conv(&mgr, "B", worktree).await;
        create_handleless_work_conv(&mgr, "C", worktree).await;
        set_context_exhausted(&mgr, "A").await;
        set_context_exhausted(&mgr, "B").await;
        wire_continuation(&mgr, "A", "B").await;
        wire_continuation(&mgr, "B", "C").await;

        // Preconditions: both intermediate nodes are continued.
        let a = mgr.db().get_conversation("A").await.expect("get A");
        let b = mgr.db().get_conversation("B").await.expect("get B");
        assert_eq!(a.continued_in_conv_id.as_deref(), Some("B"));
        assert_eq!(b.continued_in_conv_id.as_deref(), Some("C"));

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        // C (the live leaf) is being deleted/abandoned: it is excluded. No
        // non-excluded member of the chain qualifies as a live owner, so the
        // worktree must be removable.
        assert!(
            !mgr.scope_has_live_conversation_excluding(&scope, "C")
                .await
                .unwrap(),
            "deleting the live leaf C of A→B→C (A,B both continued ContextExhausted) \
             must leave no live owner — an intermediate continued ContextExhausted \
             must not keep the worktree alive"
        );
    }

    /// Counterpart to the multi-hop fix: in A→B→C with A and B both
    /// `ContextExhausted`-and-continued and C still LIVE, deleting an UNRELATED
    /// sibling must not tear the worktree down — the live leaf C owns the scope,
    /// and ownership is correctly attributed through the two-hop chain.
    #[tokio::test]
    async fn multi_hop_chain_with_live_leaf_preserves_scope_for_unrelated_delete() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        create_handleless_work_conv(&mgr, "A", worktree).await;
        create_handleless_work_conv(&mgr, "B", worktree).await;
        create_handleless_work_conv(&mgr, "C", worktree).await; // stays non-terminal (live)
        set_context_exhausted(&mgr, "A").await;
        set_context_exhausted(&mgr, "B").await;
        wire_continuation(&mgr, "A", "B").await;
        wire_continuation(&mgr, "B", "C").await;

        // Unrelated sibling on the same worktree, the one being deleted.
        create_handleless_work_conv(&mgr, "sibling", worktree).await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            mgr.scope_has_live_conversation_excluding(&scope, "sibling")
                .await
                .unwrap(),
            "the live leaf C of A→B→C owns the shared worktree; deleting an unrelated \
             sibling must not tear it down"
        );
    }

    /// A non-terminal sibling on a DIFFERENT worktree must not preserve this
    /// scope — the DB query is keyed on `worktree_path`, so unrelated live
    /// conversations are not false positives.
    #[tokio::test]
    async fn sibling_on_other_worktree_does_not_preserve_scope() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";

        create_handleless_work_conv(&mgr, "leaf", worktree).await;
        create_handleless_work_conv(&mgr, "unrelated", "/repo/.phoenix/worktrees/other").await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());

        assert!(
            !mgr.scope_has_live_conversation_excluding(&scope, "leaf")
                .await
                .unwrap(),
            "a live conversation on a different worktree does not own this scope"
        );
    }

    /// A `WorkScope::Conversation(id)` whose row is genuinely absent
    /// (`DbError::ConversationNotFound`) is `Ok(false)` — a definitive
    /// "not live", not an error. A non-NotFound DB error propagates as
    /// `Err` instead, leaving each caller to pick its policy (idle reaper
    /// preserves, cleanup cascade fails the operation). That error path
    /// needs DB fault injection the in-memory test DB does not expose, so it
    /// is exercised at the caller layer (`handlers.rs` cascade tests) and by
    /// inspection of the match arm rather than a unit test here.
    #[tokio::test]
    async fn missing_conversation_scope_is_not_live() {
        let mgr = test_manager().await;
        let scope = crate::work_scope::WorkScope::Conversation("does-not-exist".to_string());
        assert!(
            !mgr.scope_has_live_conversation(&scope).await.unwrap(),
            "an absent conversation row resolves to not-live"
        );
    }

    /// An unreadable DB makes liveness unknowable: a non-NotFound error
    /// propagates as `Err` rather than being swallowed to a bool. This is the
    /// input the idle-reaper hook maps to "preserve" (its `Err => true` arm)
    /// and the cleanup cascade maps to "fail the operation". Fault injection:
    /// closing the pool makes the worktree query fail with a non-NotFound
    /// `DbError`.
    #[tokio::test]
    async fn unreadable_db_propagates_error_for_caller_policy() {
        let mgr = test_manager().await;
        let worktree = "/repo/.phoenix/worktrees/shared";
        create_handleless_work_conv(&mgr, "owner", worktree).await;

        mgr.db().pool().close().await;

        let scope = crate::work_scope::WorkScope::Worktree(worktree.to_string());
        assert!(
            mgr.scope_has_live_conversation(&scope).await.is_err(),
            "an unreadable DB must surface as Err, not a swallowed bool — the \
             idle reaper preserves on Err, the cascade fails on Err"
        );
    }
}
