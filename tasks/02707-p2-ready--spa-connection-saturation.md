---
created: 2026-05-06
priority: p2
status: ready
artifact: ui/src/
---

# spa-connection-saturation

> **Status update (2026-05-08).** HTTP/2 (task 02705) is live in prod
> and successfully multiplexes every long-lived stream over a single
> TCP connection. The 6-connections-per-origin saturation symptoms are
> gone in practice. **This task is no longer a daily-use blocker; it
> is the structural follow-up.** The bug class — "connection lifecycle
> is enforced by component cleanup discipline rather than by
> construction" — still exists. HTTP/2 hides it; it does not solve it.
> If we ever fall back to HTTP/1.1 (proxy in path, browser without h2,
> headless test runner, etc.) the original symptoms return. The goal
> below is to eliminate the bug class.

## Original observed symptoms (preserved for context)

Before HTTP/2 landed, phoenix would become unresponsive multiple times
per day. Browser network devtools showed requests "pending forever" —
they never reached the server. Phoenix logs showed only successful
polling during the hung period. Affected requests were any user-
initiated action: posting a message, clicking "Approve task", files
pane loading, AskUserQuestion submit. Bouncing prod cleared it.

Diagnosis: HTTP/1.1 per-origin connection-pool exhaustion at the
browser. Browsers cap at 6 concurrent HTTP/1.1 connections per origin.
Each long-lived connection (SSE stream, WebSocket terminal) permanently
occupies a slot. `ss -tn` during incidents showed 7–9 ESTABLISHED
connections from the client IP to phoenix:8031 — at or past the limit
before the user had done anything. 12 SSE stream-open requests for 5
different conversation IDs were logged within 30 minutes of a fresh
session with a single tab, indicating the SPA was opening more streams
than one tab should ever need.

## What HTTP/2 fixed and what it didn't

**Fixed in practice.**
 - Browser ↔ phoenix multiplexes all streams over one TCP connection
   via ALPN-negotiated h2.
 - The 6-conn cap is no longer a ceiling: validated by 8 concurrent
   `/api/conversations/:id/stream` SSE streams + a 9th `/version` fetch
   completing in 3 ms. See task 02705 for the verification.
 - The user-facing pending-forever symptom is no longer reproducible.

**Not fixed.**
 - The SPA still opens more long-lived connections than necessary. h2
   makes the count cheap, not zero.
 - Connection lifecycle is still managed by the React component tree
   — mount opens, unmount closes — with no explicit accounting layer.
 - Slug-change navigation has a window where the old SSE+WS aren't
   yet torn down and the new ones are already opening. Under h2 this
   is invisible; under h1 it doubled the slot pressure for the
   transition.
 - SSE error → backoff → reconnect can amplify under load. h2 streams
   are cheap to retry, so the amplification is harmless today; under
   h1 it was a feedback loop.
 - `TerminalPanel` opens a WebSocket as soon as any conversation page
   is rendered, regardless of whether the user has expanded the
   panel. That was 1 wasted slot under h1; it's still a wasted
   lifecycle today.
 - There is no single place in the code that says "how many long-
   lived connections are open right now." It's emergent from
   `useEffect` deps.

## How recent merged work changed the connection-pressure profile

PR #26 (commit `8530f197`) merged tasks 02703, 08682, 08683, 08684
together. None of them are about connection accounting directly, but
each changed the lifecycle landscape this task has to navigate.

### Task 02703 — smooth-conversation-sidebar (done)

**Behavioural change.** `ConversationPage` is no longer keyed by slug;
the page survives slug changes instead of unmounting + remounting.
Connection cleanup is now driven by `useEffect([conversationId])`
dependencies inside `useConnection.ts` and `TerminalPanel.tsx` rather
than by the component subtree tearing down.

**Consequence for this task.** The cleanup paths are individually
correct (`eventSourceRef.current.close()` and `ws.close()` both fire),
but the cleanup→reopen sequence is no longer atomic. There is a
window where the OLD connection's TCP FIN hasn't been acknowledged
and the NEW connection is already opening, and the browser briefly
counts both. Pre-02703 this was hidden because the entire subtree
tore down before any new connection could open.

**Hot-path file.** `ui/src/hooks/useConnection.ts` (the
`useEffect([conversationId])` near the bottom that fires CONNECT /
DISCONNECT through the state machine).

### Task 08682 — RoutedStore + ChainPage migration (done)

**Behavioural change.** Chain Q&A state is now atom-routed by
rootConvId via the new `RoutedStore<K, S, A>` primitive. The chain
SSE subscription stayed in `ChainPage`; only the data flow moved
into the store. Lifecycle didn't change.

**Consequence for this task.** No new connections opened. But: the
store primitive is a candidate substrate for option B below — a
`ConnectionManager` could be a third specialization of
`RoutedStore` (keyed leases instead of keyed atoms) with the
routing / subscription / notify primitives reused for free.

**Hot-path files.** `ui/src/conversation/RoutedStore.ts` (the
primitive); `ui/src/api.ts:907` (`subscribeToChainStream`, still the
only chain SSE site); `ui/src/pages/ChainPage.tsx` (lifecycle owner).

### Task 08683 — epoch-stamp SSE events (done)

**Behavioural change.** Each `OPEN_SSE` mints a per-machine monotonic
epoch; every dispatched action carries the epoch; the atom rejects
stamped actions whose epoch doesn't match. Closes the cross-conv
contamination window that 02703 made structurally important.

**Consequence for this task.** Epoch tracking is **already a per-
connection identity primitive**. The infrastructure could be
repurposed for connection accounting at no incremental cost — a
`ConnectionManager` could track "how many open" simply by counting
distinct epochs whose `connection_opened` has fired but whose close
hasn't. The data is already on the wire.

**Hot-path file.** `ui/src/hooks/useConnection.ts` around the
`OPEN_SSE` effect handler (where epoch is captured into the closure
and `connection_opened` is dispatched into the atom).

### Task 08684 — single Conversation source of truth (done)

**Behavioural change.** `ConversationStore` is now the single home
for every Conversation snapshot the UI displays.
`useConversationsRefreshDriver` (mounted by `ConversationProvider`)
owns a 5 s poll over `api.listConversations()` +
`api.listArchivedConversations()`. `DesktopLayout` and
`ConversationListPage` no longer hold parallel `Conversation[]`
state.

**Consequence for this task.** Two short-lived fetches every 5 s
while the tab is visible. They release back to the keep-alive pool,
so under h2 they multiplex cheaply. **But** the driver is mounted
from a single point — the Codex review on PR #26 caught one
duplicate-mount bug already (`useConversationsRefresh()` did double
duty as accessor + driver, and any consumer that wanted `refresh`
installed a second poller). The fix split the hook into accessor
(`useConversationsRefresh`) + driver (`useConversationsRefreshDriver`)
and introduced an `__refreshInFlight` flag on the store to coalesce.
That split is the only thing preventing duplicate dispatch today; a
future mistake that mounts the driver twice would silently install
parallel pollers again. Worth a structural audit pass.

**Hot-path file.** `ui/src/conversation/useConversationsRefresh.ts`.

### Task 02708 — shutdown-sse-deadline (open, p2)

Server-side: cap graceful shutdown wait at 5–10 s so SSE clients
can't pin the binary alive after a deploy. Independent of this task;
closes a related symptom (zombie phoenix processes hoarding accepted
SSE connections after a deploy). Worth landing alongside any
structural fix here so the server side is consistent.

### Task 08685 — sweep-sync-derivation-providers-rows (open, p1)

Includes the `FileExplorerProvider` slug-keyed rework + a
`useViewport()` consolidation. Doesn't directly touch connections,
but the panel-architecture finding it captures (cooperative
invariants enforced by code discipline rather than by type) is the
same disease pattern this task surfaces in the connection layer.
If 08685 lands first it sets the precedent for the slug-keyed
structural shape that option B below would mirror.

## Current connection-opening sites (post-PR #26)

Every `new EventSource` and `new WebSocket` in `ui/src`:

| Site | Owner | Lifetime | Notes |
|---|---|---|---|
| `ui/src/hooks/useConnection.ts:278` | `ConversationPage` (one per app) | active conv slug | Closes cleanly on slug change. Epoch-stamped (08683). |
| `ui/src/api.ts:907` (`subscribeToChainStream`) | `ChainPage` | mounted chain | Closes cleanly on unmount. |
| `ui/src/components/TerminalPanel.tsx:481` | `ConversationPage` (always while on a conv page in normal phase) | mounted page | **Opens regardless of expand state.** 1 lifetime-long slot any time the user is on a conv page. |
| `ui/src/pages/SharePage.tsx:103` | `SharePage` | view lifetime | Read-only; not contended with other conv state. |
| `ui/src/components/CredentialHelperPanel.tsx:64` | auth panel | flow lifetime | Opens during auth. Short-lived in practice. |

Under h2 today this is cheap. Under h1 fallback it is exactly the
shape that produced the original incidents.

## Correct-by-construction options

The bug class is **"connection lifecycle is enforced by component
cleanup discipline rather than by construction."** Three idioms
eliminate the class. Each has different tradeoffs; this task does
*not* prescribe one — the next agent picks based on appetite for
server-side work and timeline.

### Option A — Single multiplexed channel

One SSE (or WebSocket) per tab. Server fans out conv events, chain
events, terminal output, progress events into framed messages keyed
by a `(kind, id)` tuple. Client demuxes inside one connection.

**What it eliminates structurally.** Connection count is O(1) per
tab regardless of how many things the user is watching. The 6-conn
cap stops mattering even on h1. `TerminalPanel` doesn't need its own
WS — PTY bytes flow over the same channel. There is exactly one
lifecycle to reason about.

**What it costs.** Server protocol design, a wire format, and a
client demultiplexer. The terminal byte stream becomes a framed
substream instead of a raw WebSocket; xterm.js doesn't care about
the transport but the binary-frame plumbing is non-trivial.
Backwards compatibility for the existing per-route SSE endpoints
(used by `SharePage`, `subscribeToChainStream`, `phoenix-client.py`)
is a separate concern — either keep the old endpoints alongside the
multiplexed one, or migrate every consumer.

**Verdict.** The cleanest end-state. Highest investment. Best fit
if phoenix is moving toward more concurrent stream kinds per tab
(sub-agents, live git status, watch-mode views) that would make per-
stream connections increasingly absurd even under h2.

### Option B — Connection broker with leasing API

Introduce a `ConnectionManager` (singleton, mounted by
`ConversationProvider` next to the existing
`ConversationsRefreshDriver`). Every connection-opening site moves
behind a `useConnectionLease(kind, key, factory)` hook. The manager
owns a budget, arbitrates leases, and surfaces "open count" as
observable state.

Shape:
```ts
type ConnectionKind = 'conv-sse' | 'chain-sse' | 'terminal-ws';

interface ConnectionLease<T extends EventSource | WebSocket> {
  status: 'open' | 'queued' | 'rejected';
  connection: T | null;
  release: () => void;
}

function useConnectionLease<T extends EventSource | WebSocket>(
  kind: ConnectionKind,
  key: string,
  factory: () => T,
): ConnectionLease<T>;
```

**What it eliminates structurally.** Connection count becomes a
first-class application-layer concept. Over-budget opens fail loud
instead of silently saturating the pool. Slug-change races are
impossible — the manager sees the new lease request and decides
explicitly whether to hold the old one open or pre-empt it.
Reconnect amplification is broker-controlled rather than per-hook.

**Substrate fit.** `RoutedStore<K, S, A>` (added by 08682) is the
right primitive; a `ConnectionManager` could be a third
specialization keyed by `(kind, key)` with leases as the atom
shape. Existing epoch tracking (08683) gives connection identity
for free.

**What it costs.** Every existing connection-opening site moves
behind the broker. The broker's API has to be ergonomic enough
that future authors don't bypass it (or there's a lint rule that
catches `new EventSource` outside the broker). Doesn't change the
wire protocol — works identically on h1 or h2.

**Verdict.** Best bang for the buck. No server work. Makes the bug
class impossible without committing to a multiplexed protocol.
Leaves the door open to option A later if appetite grows.

### Option C — Lazy-by-default + lint-enforced discipline

Every connection-opening component opens *only* when its purpose is
active. `TerminalPanel` mounts only on expand, unmounts on collapse.
Sub-agent panels open SSE only when the panel is open. Add an
ast-grep rule: `new EventSource` and `new WebSocket` are forbidden
outside a small allowlist of files.

**What it eliminates structurally.** Steady-state connection count
becomes proportional to active-on-screen UI rather than mounted
component count. Doesn't eliminate the class — a future component
could still mount and forget to unmount — but reduces the surface
area and makes regressions detectable via the lint rule.

**What it costs.** Smallest. No new abstractions. Each lazy-mount
is a small local change. The lint rule is one ast-grep rule.

**Verdict.** The minimum-viable structural fix. Worth doing
regardless of A or B — the lazy-mount is a free improvement, and
the lint rule catches the silent regression that any of these
options is designed to prevent.

## Recommendation framing for the next agent

Not prescriptive; pick after reviewing the tradeoffs above. One
plausible path:

1. Do option C immediately as a no-regret prerequisite. Specifically:
   make `<TerminalPanel>` lazy on expand, add the lint rule, audit
   each existing connection-opening site against it. Small PR, no
   architectural commitment.
2. Consider option B as the structural fix. Introduce the
   `ConnectionManager` as a third specialization of `RoutedStore`,
   migrate the existing four long-lived sites behind it. Mid-size
   PR. No server changes. Ergonomic test: can a new contributor
   add a per-conv telemetry stream without thinking about pool
   exhaustion? If yes, the API is right.
3. Reach for option A only if a future use case demands more than
   ~5 concurrent stream kinds per tab. Not the current need;
   phoenix's stream zoo is small. Revisit when sub-agent live-
   monitoring, live-git-status, or worktree-watcher views come
   online.

## Related triage that's still worth doing regardless of option

These are real bugs that h2 hides; they would still bite under h1
fallback. Each is self-contained and could ship before any of
A / B / C.

- **Lazy-mount `TerminalPanel`.** `ui/src/pages/ConversationPage.tsx`
  defines `showTerminal = !!conversationId && phase !== 'terminal'
  && phase !== 'context_exhausted'`. Add `&& !terminalPane.collapsed`
  so the panel doesn't render (and the WS doesn't open) when
  collapsed. Single-line change with a measurable connection-count
  drop.
- **Cap reconnect retries on apparent saturation.** In
  `ui/src/hooks/connectionMachine.ts`, if `attempt > 3` and every
  prior attempt errored within ~500 ms, treat it as pool exhaustion
  rather than server failure: hold for 30 s before retrying.
  Reduces feedback-loop pressure during any future h1 incident.
- **Instrument connection count in dev mode.** A debug overlay (or
  just a `console.debug` on every open / close) that shows the
  current long-lived count. Catches future regressions without
  waiting for a prod incident.
- **Audit `ConversationsRefreshDriver` for double-mount.** The Codex
  review on PR #26 caught one duplicate-mount bug already (fixed by
  splitting the hook into accessor + driver, plus
  `__refreshInFlight` on the store). Add a runtime assertion: panic
  in dev if the driver hook fires its initial poll twice on the
  same store instance.

## Acceptance for the structural fix

Whichever option lands:

- A new contributor adding a per-conv stream (e.g. live git status)
  must not be able to leak a connection or silently exceed an
  implicit budget. Test for this by reading the diff of a synthetic
  PR that adds such a stream and asking "could this regress
  connection accounting without anyone noticing?".
- An ast-grep / lint rule prevents `new EventSource` and `new
  WebSocket` outside a known allow-list of files (the broker, or
  the multiplexer client).
- Under h1 fallback (e.g. h2 disabled in dev), the original
  pending-forever symptom does not reproduce on the QA workflow
  documented below.
- Under h2 (current prod), no behaviour changes.

## QA workflow for any fix

From the workspace, run a 30-minute session covering:

1. Open a conversation. Confirm `ss -tn | grep :8031 | wc -l` shows
   the expected steady-state count for the chosen design.
2. Navigate between 5 different conversations rapidly.
3. Open a chain page. Submit a question. Let it stream.
4. Expand the terminal, send a few commands, collapse it.
5. Force-disable h2 in dev (`PHOENIX_TLS=off` or equivalent) and
   repeat. Original symptom must not reproduce.

## Notes

Filed 2026-05-06 as the SPA-side companion to 02705 (HTTP/2).
Updated 2026-05-08 after PR #26 (02703 + 08682 / 83 / 84) landed and
h2 went live in prod — reframed as the structural follow-up rather
than the daily-use blocker. Demoted from p0 to p2 since h2 acts as
the hotfix.
