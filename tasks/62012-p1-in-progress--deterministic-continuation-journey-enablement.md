# Deterministic continuation journey enablement

Child of parent task 17003.

Commission A. Exact base for the eventual implementation must be `382a8aa5de1967f91839c74339a847a04e4778d3`.

Enable one deterministic continuation journey that proves the shipped ProductConversation continuation path end to end without fabricated state or test-only production toggles.

## Scope

Owners are restricted to:
- `crates/phoenix-llm/src/mock.rs`
- `tests/e2e/run.py`

Do not edit any other production or test files unless hard necessity is demonstrated before edits.

## Deliverable

1. Add one deterministic mock-provider scenario in `crates/phoenix-llm/src/mock.rs` that reaches `ContextWindowExceeded` through a normal turn.
   - The scenario must be selectable like existing mock scenarios.
   - The exhaustion must arise from the normal mocked LLM turn path, not from a fault injection endpoint, hidden admin toggle, direct DB mutation, or bypass of shipped turn handling.

2. Add one real-binary HTTP/SSE end-to-end test in `tests/e2e/run.py` that:
   - starts the real Phoenix binary;
   - creates a ProductConversation through shipped APIs;
   - drives the conversation until it naturally exhausts via the deterministic mock scenario;
   - continues the conversation through shipped APIs;
   - waits on real state/message/SSE signals only, with no arbitrary sleeps;
   - proves all of the following after continuation settles:
     - exactly one ProductConversation exists for the journey;
     - typed handoff occurs exactly once;
     - the successor is the latest member and writable;
     - the canonical aggregate route resolves for the journey.

## Non-goals / prohibitions

- No production fault endpoint or test-only production toggle.
- No fabricated DB state or direct invalid inserts.
- No arbitrary sleeps.
- No production behavior changes beyond the bounded mock scenario needed to drive the journey.
- No widening into unrelated continuation, ProductConversation, persistence, or routing refactors.
- Never merge or deploy as part of this commission.

## Verification expectations for implementation

After approval and implementation:
- run focused checks for the touched area;
- run the relevant full checks needed to validate the exact-head result;
- obtain `phoenix-adversarial-review` plus one independent review;
- keep the PR immutable once exact-head review/CI begins;
- resolve every review thread before completion.

## Investigation notes

Initial duplicate-work check found:
- parent task `17003` exists and is ProductConversation-focused;
- no existing task in `tasks/` appears to already own this exact deterministic continuation-journey enablement slice;
- `tests/e2e/run.py` already hosts real-binary HTTP/SSE mock-provider end-to-end scenarios, making it the right acceptance-test home;
- `crates/phoenix-llm/src/mock.rs` already hosts marker-driven deterministic mock scenarios, making it the right bounded place to add one exhaustion scenario.

## Acceptance checklist

- [ ] One deterministic mock scenario produces `ContextWindowExceeded` via a normal turn.
- [ ] One HTTP/SSE e2e covers exhaust → continue using the shipped API surface only.
- [ ] The e2e uses signal-based waiting only; no arbitrary sleeps.
- [ ] The e2e proves one ProductConversation, one typed handoff, latest writable successor, and canonical aggregate routing.
- [ ] Only `crates/phoenix-llm/src/mock.rs` and `tests/e2e/run.py` change unless hard necessity is demonstrated first.
