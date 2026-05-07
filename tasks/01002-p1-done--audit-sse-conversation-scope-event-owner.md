---
created: 2026-05-07
priority: p1
status: done
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
| 1 | `ConversationPage` `conversationIdForSSE` debounce effect | removed | timer after route switch | fixed | Deleted page-level 100ms debounce; `ConversationPage` now passes `conversationId` directly to `useConnection`. No delayed page identity state remains. |
| 2 | `useConnection` subscription lifecycle | captured `convId` + machine `epoch` + `EventSource` instance | stale EventSource event, close/reconnect race | fixed | `useConnection` wraps every EventSource listener in an owner check before parse/dispatch. `useConnection.test.ts` covers stale A message after B switch. |
| 3 | SSE wire init handling | captured `convId` + epoch + EventSource instance; atom `connectionEpoch` | late init overwrites active atom / visible connection state | fixed | Stale init returns before `SSE_OPEN` or `sse_init`. Test: stale A init after switch leaves B-visible connection state `connecting`. |
| 4 | SSE message handling | captured listener owner + reducer epoch | cross-conversation message append | fixed | Stale handler returns before dispatch; reducer still rejects stale epoch defense-in-depth. Test: stale A message cannot land in B atom. |
| 5 | SSE state/done/error handling | captured listener owner + reducer epoch | wrong active phase / status | fixed | All EventSource event types use same guarded `on(...)`; native error cannot drive current machine after owner changes. Test: stale A native error leaves B-visible state unchanged. |
| 6 | `useConnection` offline/reconnecting/backoff timers | captured timer `conversationId` + epoch | stale timer mutates B connection state | fixed | Retry and reconnected-display timers check owner before dispatching machine inputs; cleanup still cancels. Fake-timer test covers stale retry timer after switch. |
| 7 | `useConnection` exposed connection state in `ConversationPage` | single `useConnection` machine scoped to current `conversationId` | wrong banner/statebar status | fixed | Removed unused atom-level connection mirror; StateBar still reads only guarded `connectionInfo.state`. |
| 8 | `cacheDB.putMessages` from streamed messages | atom messages only, guarded upstream by listener owner + epoch | cache poisoning / wrong conversation persistence | already safe | Cache effect writes `atom.messages`; stale SSE cannot mutate wrong atom. No separate active-route lookup at write time. |
| 9 | `cacheDB.putConversation` metadata updates during stream | atom conversation only, guarded upstream by listener owner + epoch | cache poisoning / stale metadata | already safe | Cache effect writes `atom.conversation`; stale `conversation_update` / `init` cannot cross owner. |
| 10 | `useMessageQueue` reconciliation vs streamed `message_id`s | `conversationId` localStorage key + current atom messages | queue leakage / false pending removal | fixed by upstream | Queue drain trusts `connectionInfo.state`; stale connection state now cannot flip to connected. Reconciliation derives only from current `queuedMessages` and current atom message IDs. |
| 11 | `markFailedRef` / `dismissRef` / retry effects in `ConversationPage` | current `conversationId` closure + per-conversation queue key | stale callback on active conversation | needs follow-up | Not SSE-originated. Async send failure after route switch can still call current refs; capture separately if observed. |
| 12 | cancellation / continuation / upgrade actions while switching | current `conversationId` closure | stale action target | not applicable | User actions target current render's `conversationId`; not an SSE/subscription path. |
| 13 | chain SSE (`subscribeToChainStream`) | `rootConvId` effect owner + ChainStore routed key | sibling pattern comparison | already safe | ChainPage closes on root change/unmount and demuxes by `chain_qa_id`; no shared current-conversation machine. Existing ChainPage tests cover route/store isolation. |
| 14 | browser live-view events coupled to messages | guarded `browser_session_state` SSE + slug-scoped provider | late browser activation | fixed | Browser session event uses guarded listener + reducer epoch; `BrowserViewStateProvider` is `scopeKey={slug}`. |

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
