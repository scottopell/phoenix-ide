# Debug and fix transcript jitter with repeated render units

## Problem

On mobile, scrolling an active conversation can rarely enter a rapid up/down oscillation. A captured frame shows the same apparent assistant/tool card three times while the transcript jitters. The conversation is using the latest-history window (`Load earlier history` is present) and is actively running.

The symptom crosses two contracts:

- the render-unit pipeline must supply one globally unique identity per item;
- `VirtualTranscript` uses that identity for React reconciliation, measured extents, row-element refs, layout offsets, and anchor restoration.

A duplicate key can therefore both repeat/mis-reconcile content and make competing row measurements repeatedly move the anchor. Alternatively, the cards may have distinct IDs but duplicate persisted content, in which case the transcript is rendering valid input and the defect is upstream. Diagnosis must distinguish these cases before selecting the fix.

## Evidence and current hypotheses

- `VirtualTranscript` renders each item once from a contiguous `items.slice(...)`; overscan cannot manufacture three copies by itself.
- `MessageList` concatenates `historicalUnits` and `tailUnits` without validating uniqueness.
- A streaming unit intentionally shares the eventual finalized assistant `message_id` so finalization can be an in-place transition. `sse_message` atomically adds the finalized message and clears `streamingBuffer`, but `messages` and `useStreamingRequestId()` arrive through separate subscriptions/render boundaries; the required single-commit transition needs an integration regression test.
- `VirtualTranscript` stores measurements, row elements, ref callbacks, and anchors by key. Duplicate keys make its physical state ambiguous and can cause `ResizeObserver` anchor correction to alternate between different DOM rows.
- SQLite enforces unique `message_id`, and the normal page/atom merge functions dedupe by `message_id` and `sequence_id`, making simple page overlap less likely.
- Three distinct assistant message IDs with identical content remain possible and must be checked rather than semantically deduped in the UI.
- The recent custom `VirtualTranscript` and latest-history work are the likely regression surface; the older scroll policy may amplify geometry churn but should not be changed until identity and observer behavior are measured.

## Plan

### 1. Make the failure observable

- Add development/test diagnostics at the `MessageList` → `VirtualTranscript` boundary that report, without message body contents:
  - conversation ID;
  - duplicate render-unit keys and each unit kind/index;
  - source `message_id` and `sequence_id` where applicable;
  - whether historical and streaming units coexist under one key.
- Add focused virtualizer diagnostics/tests for repeated key measurements and anchor writes. Do not ship noisy per-scroll production logs.
- If available in the affected database, inspect the conversation’s message IDs, sequence IDs, types, and content hashes to determine whether the screenshot represents duplicate identities or distinct persisted messages with equal content.

### 2. Reproduce both candidate classes

- Add a MessageList fixture/test for an active mobile-sized transcript with latest-history coverage, a tool-bearing assistant turn, and streaming/finalization transitions while the reader scrolls.
- Exercise the store update where `sse_message` simultaneously appends the finalized message and clears the streaming buffer; assert every committed unified item list has unique keys and the finalized row occurs once.
- Add a `VirtualTranscript` regression with duplicate-key input of unequal measured heights to demonstrate or rule out `ResizeObserver`/anchor oscillation.
- Add a control case with distinct message IDs and identical visible content; assert it remains stable and is not incorrectly content-deduplicated.
- Cover touch/momentum reader ownership and a latest-history prefix expansion so tail-follow and continuity restoration cannot fight the user during the scenario.

### 3. Fix the proven source structurally

If a historical/streaming transition can expose both units:

- derive one discriminated unified render-unit state where the streaming and finalized forms are mutually exclusive by construction, rather than relying on synchronized independent inputs;
- preserve the intended stable key across the transition;
- strengthen `render_units.allium` from historical-only uniqueness to uniqueness across the complete rendered list.

If duplicate identities enter from history/SSE merge:

- fix the responsible merge boundary and define one canonical identity conflict policy shared by page loading and atom reconciliation;
- retain diagnostics for impossible identity conflicts at the boundary.

If the rows have distinct identities and were persisted repeatedly:

- trace the runtime/provider turn persistence that created them and fix idempotency there;
- do not hide valid distinct messages with UI content-hash deduplication.

In all cases:

- make `VirtualTranscript` reject or safely quarantine duplicate keys before they enter key-indexed measurement state, so malformed input cannot create an infinite visual feedback loop;
- ensure anchor correction has one physical row owner per key and does not alternate DOM scroll writes during a user-owned touch/momentum gesture.

### 4. Verify

- Run targeted render-unit, MessageList, VirtualTranscript, scroll-machine, history-expansion, and conversation-atom tests.
- Run the mobile fixture in a real browser at the captured viewport class, including active streaming, upward scrolling, momentum after finger release, and `Load earlier history`.
- Check for React duplicate-key warnings, repeated ResizeObserver corrections, unexpected tail-follow writes, duplicated cards, and any up/down oscillation.
- Run `allium check` for touched Allium specs and `./dev.py check`.

## Acceptance criteria

- The complete list passed to `VirtualTranscript` has globally unique keys in every committed state.
- Streaming → finalized assistant transition displays exactly one row and preserves stable identity.
- Distinct messages with identical content remain distinct and scroll stably.
- Malformed duplicate-key input cannot cause a ResizeObserver/scroll-anchor feedback loop.
- Mobile touch/momentum scrolling remains user-owned; history expansion and active tail updates do not produce rapid alternating scroll corrections.
- Regression coverage captures the identity/geometry combination implicated by the screenshot.
