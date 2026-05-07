---
created: 2026-05-07
priority: p1
status: ready
artifact: ui/src
---

Audit every SSE / streaming / subscription path for conversation-scope ownership bugs.

We have observed intermittent issues when switching between conversations that are both concurrently sending SSE updates. The failures are timing/network dependent: late events, reconnects, init messages, or timers from conversation A can appear to affect conversation B after navigation. This task is the SSE-focused sibling of task 05002's UI-state scope audit.

## Symptom shape

User is viewing `/c/<slug-A>` while A is streaming. They navigate to `/c/<slug-B>` while B is also streaming or reconnecting. Under certain ordering/timing conditions, stale work created for A leaks into B's visible state, connection state, cache, queue reconciliation, or affordances.

Examples to hunt:
- A's EventSource emits after route switch and mutates B-visible state.
- A's reconnect/error handler toggles B's connection banner/state.
- A's late init/message/done event races B's REST load or B's SSE init.
- A's event writes messages/cache entries under the wrong active identity.
- A's timers/debounced connection setup fire after B becomes active.

## Core invariant

An SSE event, timer, or async continuation created under conversation A must not commit state under conversation B.

This should be enforced structurally wherever possible: event handling should carry an explicit owner key (`conversationId`, and where useful `slug` plus a subscription generation/epoch) and drop stale work before dispatching visible state updates.

## Systematic checklist

Use the table as the entry system. For every candidate, classify it and record evidence/status.

| # | Candidate | Owner key today | Risk class | Status | Notes / fix / test |
| --- | --- | --- | --- | --- | --- |
| 1 | `ConversationPage` `conversationIdForSSE` debounce effect | `atom.conversationId` + timer cleanup | timer after route switch | TODO | Verify delayed `setConversationIdForSSE` from A cannot connect under B. Add fake timer test if missing. |
| 2 | `useConnection` subscription lifecycle | TBD | stale EventSource event, close/reconnect race | TODO | Audit create/cleanup order, generation guards, and whether handlers can dispatch after cleanup. |
| 3 | SSE wire init handling | TBD | late init overwrites active atom | TODO | Verify `init` is scoped to the subscription conversation id and cannot clobber current route state. |
| 4 | SSE message handling | TBD | cross-conversation message append | TODO | Verify message events route to the correct conversation atom/cache by event/subscription owner, not current page. |
| 5 | SSE state/done/error handling | TBD | wrong active phase / status | TODO | Verify phase transitions are scoped and stale events are dropped. |
| 6 | `useConnection` offline/reconnecting/backoff timers | TBD | stale timer mutates B connection state | TODO | Fake timers should cover A reconnect/backoff firing after route switch to B. |
| 7 | `useConnection` exposed connection state in `ConversationPage` | TBD | wrong banner/statebar status | TODO | Ensure UI reflects only active subscription. |
| 8 | `cacheDB.putMessages` from streamed messages | TBD | cache poisoning / wrong conversation persistence | TODO | Verify writes use message/conversation owner, not active route at write time. |
| 9 | `cacheDB.putConversation` metadata updates during stream | TBD | cache poisoning / stale metadata | TODO | Verify owner identity. |
| 10 | `useMessageQueue` reconciliation vs streamed `message_id`s | `conversationId` localStorage key | queue leakage / false pending removal | TODO | Confirm queued messages only reconcile against same conversation's atom messages. |
| 11 | `markFailedRef` / `dismissRef` / retry effects in `ConversationPage` | current hook refs | stale callback on active conversation | TODO | Audit effect deps and callback ownership. |
| 12 | cancellation / continuation / upgrade actions while switching | current `conversationId` closure | stale action target | TODO | Ensure handlers cannot target prior conversation after navigation. |
| 13 | chain SSE (`subscribeToChainStream`) | `rootConvId` | sibling pattern comparison | TODO | Use ChainPage's routed-store/dead-atom pattern as a reference and verify no analogous chain leak remains. |
| 14 | browser live-view events coupled to messages | message scan + provider state | late browser activation | TODO | Verify stale browser tool events from A cannot auto-open B's browser panel. |

Add rows as new candidates appear. Do not remove rows; mark `not applicable`, `already safe`, `fixed`, or `needs follow-up` with evidence.

## Test matrix

Build a fake/controllable EventSource harness (or reuse an existing one) and cover at least:

- A emits `message` after switching to B: B must not display A's message.
- A emits `init` after B's REST load or B's init: B must keep B's conversation/messages/phase.
- A emits `error`/reconnect after B connected: B's connection banner/state must not show A's reconnect.
- A's delayed connection timer fires after route switch to B: no A subscription should become active.
- A and B stream interleaved events: each atom/cache/queue updates only its own owner.
- Unmount/route switch while backoff/retry timer is pending: timer is cancelled or generation-guarded.

## Mechanism requirement

Ship one durable mechanism, not only per-site fixes. Candidate mechanisms:

- Subscription generation/epoch guard inside `useConnection`.
- A `useScopedSseSubscription(scopeKey, subscribe, handlers)` helper.
- Explicit owner-key validation at the reducer/store boundary.

Pick the smallest mechanism that makes stale event commits hard to write accidentally.

## Acceptance

- The checklist table is completed with every SSE/streaming candidate classified.
- At least one shared guard/mechanism prevents stale subscription events or timers from committing after scope changes.
- Regression tests cover late event, late init, reconnect/backoff, and interleaved A/B streaming scenarios.
- Any discovered cross-conversation cache/queue/message leak is fixed or captured as an explicit follow-up with evidence.
- `./dev.py check` passes.
