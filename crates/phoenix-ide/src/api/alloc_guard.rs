//! Allocation-regression guards for the SSE serialization hot paths.
//!
//! These paths run per-message (Init / `Message` events) and per-token
//! (`Token` events) — the busiest allocation sites the API has. A change that
//! reintroduces a redundant clone or a whole-aggregate serialization where a
//! field-level one suffices is invisible to correctness tests (the bytes on
//! the wire are identical) but doubles the allocator traffic. These guards
//! make that class of regression fail CI.
//!
//! # How it works
//!
//! A thread-local counting allocator wraps the system allocator. Counting is
//! off by default and switched on only for the current thread inside
//! [`measure`], so the guards are accurate even though `cargo test` runs the
//! suite in parallel — concurrent allocations on other threads are never
//! attributed to a measured closure. The trait's default `realloc` routes
//! through `alloc`/`dealloc`, so `Vec`/`String`/serialization-buffer growth is
//! counted too.
//!
//! # Budgets are tripwires, not specs
//!
//! Each ceiling sits well above the current measured cost and below the cost
//! of the anti-pattern it guards against (cited in each test). It exists to
//! catch a doubling, not to pin an exact count — minor allocation shifts from
//! a `serde` bump should not trip it. Re-measure with the ignored
//! [`report_current_allocations`] test and widen a ceiling deliberately if a
//! legitimate change pushes past it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Counting is enabled per-thread only while inside [`measure`]. `const`
    /// init keeps the thread-local on the no-lazy-init, no-destructor path so
    /// accessing it from inside the allocator cannot recurse into allocation.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() && ACTIVE.with(Cell::get) {
            ALLOCS.with(|c| c.set(c.get() + 1));
            BYTES.with(|b| b.set(b.get() + layout.size() as u64));
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy)]
pub(crate) struct AllocStats {
    pub allocations: u64,
    pub bytes: u64,
}

/// Run `f` with per-thread allocation counting enabled and return its result
/// alongside the allocations it performed. Allocations on other threads (and
/// the eventual drop of the returned value) are not counted.
///
/// Cleanup is unconditional (a `Drop` guard), so a panic inside `f` cannot
/// leave the thread armed. Nesting composes: an inner `measure` restores the
/// outer's prior counters and folds its own count back into them, so the outer
/// still observes allocations made inside the inner closure.
pub(crate) fn measure<R>(f: impl FnOnce() -> R) -> (R, AllocStats) {
    /// On drop, fold this scope's counts back into the saved parent totals and
    /// restore the parent's `ACTIVE` flag — runs on both normal return and
    /// unwind.
    struct Restore {
        prev_active: bool,
        prev_allocs: u64,
        prev_bytes: u64,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            let allocs = ALLOCS.with(Cell::get);
            let bytes = BYTES.with(Cell::get);
            ALLOCS.with(|c| c.set(self.prev_allocs.wrapping_add(allocs)));
            BYTES.with(|c| c.set(self.prev_bytes.wrapping_add(bytes)));
            ACTIVE.with(|a| a.set(self.prev_active));
        }
    }

    let restore = Restore {
        prev_active: ACTIVE.with(Cell::get),
        prev_allocs: ALLOCS.with(Cell::get),
        prev_bytes: BYTES.with(Cell::get),
    };
    ACTIVE.with(|a| a.set(true));
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|b| b.set(0));

    let result = f();
    let stats = AllocStats {
        allocations: ALLOCS.with(Cell::get),
        bytes: BYTES.with(Cell::get),
    };
    drop(restore);
    (result, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::wire::{EnrichedMessage, SseWireEvent};
    use crate::db::{Message, MessageContent, MessageType, UsageData};
    use crate::llm::ContentBlock;
    use crate::runtime::SseEvent;
    use chrono::Utc;
    use serde_json::json;

    /// A representative mid-length agent turn: interleaved text and bash
    /// `tool_use` blocks plus the `display_data` the enrichment path merges.
    /// Big enough that whole-`Message` serialization is meaningfully costlier
    /// than content-only, so the enrichment guard can tell them apart.
    fn agent_message_fixture() -> Message {
        let paragraph = "a paragraph of agent reasoning roughly the length of a \
             real explanatory block in a turn, carried far enough to weigh on \
             the wire. "
            .repeat(3);
        let mut blocks = Vec::new();
        for i in 0..6 {
            blocks.push(ContentBlock::Text {
                text: format!("Step {i}: {paragraph}"),
            });
            blocks.push(ContentBlock::ToolUse {
                id: format!("tool-{i}"),
                name: "bash".to_string(),
                input: json!({
                    "cmd": format!("cd /srv/app && ./run.sh --step {i} --verbose"),
                    "timeout_ms": 120_000,
                    "env": { "RUST_LOG": "debug", "STEP": i },
                }),
            });
        }
        let bash: Vec<_> = (0..6)
            .map(|i| json!({ "tool_use_id": format!("tool-{i}"), "display": format!("./run.sh --step {i}") }))
            .collect();
        Message {
            message_id: "msg-guard".to_string(),
            conversation_id: "conv-guard".to_string(),
            sequence_id: 7,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(blocks),
            display_data: Some(json!({ "bash": bash })),
            usage_data: Some(UsageData {
                input_tokens: 1200,
                output_tokens: 800,
                cache_creation_tokens: 0,
                cache_read_tokens: 4096,
            }),
            created_at: Utc::now(),
        }
    }

    fn token_event_fixture() -> SseEvent {
        SseEvent::Token {
            sequence_id: 42,
            text: "a typical streamed token chunk of a dozen-ish words".to_string(),
            request_id: "req-0123456789abcdef".to_string(),
        }
    }

    /// Naive baseline kept *only* as a calibration reference: serialize the
    /// whole `Message` and clone out the `content` sub-tree. This is the
    /// anti-pattern the enrichment guard's ceiling sits below; it is not used
    /// by any guard assertion directly.
    fn enrich_via_whole_message(msg: &Message) -> serde_json::Value {
        let full = serde_json::to_value(msg).unwrap_or(serde_json::Value::Null);
        full.get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }

    /// Per-message enrichment (`EnrichedMessage::from`) must not regress to
    /// whole-`Message` serialization. The shipping content-only path sits well
    /// below the whole-`Message` content extraction it replaced; a reverted
    /// `from()` (which still clones `display_data` and runs the bash merge on
    /// top) lands above the ceiling. The ceiling guards that relationship, not
    /// an exact count — current measured values live in the ignored
    /// `report_current_allocations` test, deliberately not duplicated here.
    #[test]
    fn enriched_message_from_stays_content_only() {
        let msg = agent_message_fixture();
        // Warm any one-time lazy init so it isn't charged to the measurement.
        let _ = measure(|| EnrichedMessage::from(&msg));

        let (_enriched, stats) = measure(|| EnrichedMessage::from(&msg));
        assert!(
            stats.allocations <= 260,
            "EnrichedMessage::from allocated {} times (ceiling 260); \
             whole-Message serialization was likely reintroduced — enrich only \
             msg.content, not the whole envelope (run report_current_allocations \
             for current costs)",
            stats.allocations
        );
    }

    /// The per-token wire path runs once per streamed token — the hottest
    /// allocation site in the API. Serializing a `Token` to its `data:` string
    /// should stay a handful of allocations (output buffer + a little serde
    /// scratch); a regression here multiplies across every token of every
    /// turn.
    #[test]
    fn token_wire_serialization_stays_cheap() {
        let _ = measure(|| {
            let wire: SseWireEvent = token_event_fixture().into();
            serde_json::to_string(&wire).unwrap()
        });

        let (_data, stats) = measure(|| {
            let wire: SseWireEvent = token_event_fixture().into();
            serde_json::to_string(&wire).unwrap()
        });
        assert!(
            stats.allocations <= 12,
            "Token -> wire -> string allocated {} times (>12); the per-token \
             hot path regressed",
            stats.allocations
        );
    }

    /// Sanity check on the harness itself: a no-op closure performs no
    /// counted allocations, and an obvious allocation is counted. Guards the
    /// guards.
    #[test]
    fn measure_counts_only_inside_the_closure() {
        let (_unit, idle) = measure(|| {});
        assert_eq!(idle.allocations, 0);

        let (v, stats) = measure(|| vec![1u8, 2, 3]);
        assert_eq!(v, vec![1, 2, 3]);
        assert!(stats.allocations >= 1);
    }

    /// Nesting composes: the inner `measure` reports only its own allocations,
    /// and the outer still observes the inner closure's allocations folded in.
    #[test]
    fn measure_nests_without_corrupting_outer() {
        let mut inner_allocs = 0;
        let (_unit, outer) = measure(|| {
            std::hint::black_box(vec![0u8; 16]); // outer-only allocation
            let (_v, inner) = measure(|| std::hint::black_box(vec![0u8; 16]));
            inner_allocs = inner.allocations;
        });
        assert!(inner_allocs >= 1, "inner measured nothing");
        assert!(
            outer.allocations > inner_allocs,
            "outer ({}) should include both its own and the inner closure's \
             allocations ({})",
            outer.allocations,
            inner_allocs
        );
    }

    /// Cleanup is unconditional: a panicking closure must not leave the thread
    /// armed. Asserts `ACTIVE` is disarmed after the unwind.
    #[test]
    fn measure_disarms_after_panic() {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // silence the expected panic
        let caught =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| measure(|| panic!("boom"))));
        std::panic::set_hook(prev_hook);

        assert!(caught.is_err(), "closure was expected to panic");
        assert!(
            !ACTIVE.with(Cell::get),
            "ACTIVE stayed armed after a panicking measure"
        );
    }

    /// Print current allocation costs for tuning budgets. Ignored — run with:
    /// `cargo test -p phoenix_ide report_current_allocations -- --ignored --nocapture`
    #[test]
    #[ignore = "diagnostic; run with --ignored --nocapture to re-tune budgets"]
    fn report_current_allocations() {
        let msg = agent_message_fixture();
        let _ = measure(|| EnrichedMessage::from(&msg));

        let (_e, enrich) = measure(|| EnrichedMessage::from(&msg));
        let (_w, whole) = measure(|| enrich_via_whole_message(&msg));
        let (_t, token) = measure(|| {
            let wire: SseWireEvent = token_event_fixture().into();
            serde_json::to_string(&wire).unwrap()
        });
        println!("allocation costs (allocs, bytes):");
        println!(
            "  EnrichedMessage::from (content-only): {:>4}, {:>7}",
            enrich.allocations, enrich.bytes
        );
        println!(
            "  whole-Message baseline (anti-pattern):{:>4}, {:>7}",
            whole.allocations, whole.bytes
        );
        println!(
            "  Token -> wire -> string:              {:>4}, {:>7}",
            token.allocations, token.bytes
        );
    }
}
