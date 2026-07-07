# iOS Client

## User Story

As a Phoenix user away from my desk, I need a native iOS client that stays
usable on an unreliable connection — reading history, navigating
conversations, and composing messages while offline — so that a network gap
(subway, elevator, dead zone) never loses my words or strands me on a
blank screen.

The iOS client is a deliberately simplified companion to the web UI: it
covers conversations, messages, tool activity, and sending. Server-side
capabilities that require a live interactive channel (terminal, diff
viewer, chains, file browser) are out of scope.

## Requirements

### REQ-IOS-001: Offline-First Rendering

WHEN the app opens with no network connectivity
THE SYSTEM SHALL render the cached conversation list
AND allow navigation into any previously-viewed conversation
AND render that conversation's cached message history

WHEN cached data is shown and its age exceeds a staleness threshold
THE SYSTEM SHALL indicate the cache age inline

**Rationale:** The core robustness promise. Cached state renders before any
network I/O; the network only ever improves the view.

---

### REQ-IOS-002: Offline Message Queue

WHEN the user submits a non-empty message
THE SYSTEM SHALL persist a queue entry to disk before attempting delivery
AND render the entry optimistically in the transcript with its queue status

WHEN delivery fails at the transport level (no route, timeout, connection reset)
THE SYSTEM SHALL keep the entry queued
AND retry automatically on the next delivery trigger

WHEN the server definitively rejects the send (an HTTP error response)
THE SYSTEM SHALL mark the entry failed
AND offer explicit Retry and Discard affordances

WHEN the app restarts with queued entries on disk
THE SYSTEM SHALL rehydrate only entries belonging to the viewed conversation

The queue implements the client-side delivery contract of
`specs/user_message_queue/user_message_queue.allium` (enqueue-before-POST,
`message_id = localId`, reconciliation against authoritative history,
union-without-duplicates rendering), with two platform deviations:

1. Transport-level failures do not transition entries to `failed`; they
   remain `pending` for automatic redelivery. Mobile connectivity loss is
   the common case, not an exceptional one, and resends are safe by
   idempotency (REQ-IOS-003).
2. The causal proof for `recoverable_inconsistency` is approximated by
   time: a server-accepted non-steering entry unreflected in history after
   a bounded live-connection window is surfaced with a retry affordance.

**Rationale:** A message written in a tunnel must survive the tunnel, the
app being backgrounded, and the phone rebooting.

---

### REQ-IOS-003: Idempotent Delivery

WHEN a queue entry is delivered
THE SYSTEM SHALL send the entry's local id as the `message_id` field
AND treat any resend of the same entry as safe

WHEN the same entry would be sent concurrently
THE SYSTEM SHALL prevent overlapping in-flight sends of that entry

**Rationale:** The server deduplicates on `message_id`, making at-least-once
delivery converge to exactly-once. Aggressive retrying is then free of
duplicate-message risk.

---

### REQ-IOS-004: Automatic Queue Drain

WHEN connectivity is restored (path monitor transition)
OR the SSE stream (re)connects and delivers an init snapshot
OR the app returns to the foreground
OR an agent turn completes
THE SYSTEM SHALL attempt delivery of all sendable queued entries, oldest first

**Rationale:** The user should never have to remember to resend. Every
event that plausibly changes deliverability triggers a drain; idempotency
makes over-triggering harmless.

---

### REQ-IOS-005: Live Updates with Resilient Reconnection

WHEN a conversation is open and connectivity exists
THE SYSTEM SHALL consume the conversation SSE stream (init, message,
message_updated, state_change, token, agent_done, steer_message_queued,
error events)
AND apply events through a sequence-guarded reducer (events at or below the
current sequence floor are dropped)

WHEN the stream drops
THE SYSTEM SHALL reconnect with exponential backoff and jitter, bounded by
a maximum interval
AND treat each reconnect's init snapshot as a full resync, replaying the
snapshot's pending events (server replay ring) through the same reducer

WHEN the init snapshot reports its pending events as truncated
THE SYSTEM SHALL render the durable snapshot only and await live events

WHEN the device reports no network path
THE SYSTEM SHALL suspend reconnect attempts until the path returns rather
than burning backoff cycles

**Rationale:** Mirrors the web client's connection machine against the
server contract in `specs/sse_wire/sse_wire.allium`; the init-as-resync
design means the client never needs gap detection.

---

### REQ-IOS-006: Steering Visibility

WHEN a send is accepted while the agent is busy (steering response)
THE SYSTEM SHALL show the entry as queued for after the current turn
AND clear it only when the message appears in authoritative history

**Rationale:** Sending mid-turn is normal on mobile; the user needs to see
that the message was accepted but deferred, not lost.

---

### REQ-IOS-007: Connectivity Transparency

WHEN the device is offline
THE SYSTEM SHALL show a persistent offline indicator
AND show, on the send control, that submission will queue rather than send

WHEN the stream is disconnected but the device is online
THE SYSTEM SHALL show a reconnecting indicator inline with the conversation

**Rationale:** Trust in the queue requires always knowing which mode the
app is in. Indicators are inline and quiet when everything is healthy.

---

### REQ-IOS-008: Authentication and Transport

WHEN a server password is configured
THE SYSTEM SHALL authenticate every request with `Authorization: Bearer
<password>`
AND store the password in the iOS Keychain

WHEN the server presents a self-signed certificate and the user has enabled
the trust toggle for that server configuration
THE SYSTEM SHALL accept the certificate; otherwise standard trust
evaluation applies

**Rationale:** Matches the non-browser client auth scheme (the phoenix-auth
cookie is a browser session token, not a client credential) and the
self-signed TLS posture of typical Phoenix deployments.

---

### REQ-IOS-009: Conversation Creation

WHEN the user creates a conversation
THE SYSTEM SHALL validate the working directory against the server with
inline validity feedback
AND allow choosing a model from the server's available models
AND require connectivity (creation is not queued offline)

**Rationale:** Creation requires server-side validation and id minting;
queuing it offline would fabricate state the server may reject. The
simplification is acceptable because the offline-robustness promise centers
on existing conversations.

---

### REQ-IOS-010: Message Rendering

WHEN rendering authoritative history
THE SYSTEM SHALL render user text, agent text blocks, collapsed tool-use
cards (name + input summary, expandable), and tool results (status +
expandable output, error-tinted on failure)
AND stream in-flight agent text from token events
AND fall back to a compact JSON rendering for unrecognized content shapes
rather than omitting them

**Rationale:** Readable transcripts with progressive disclosure; unknown
shapes must degrade visibly, not vanish (omission is data loss).
