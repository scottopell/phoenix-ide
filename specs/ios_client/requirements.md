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

WHEN that persistence point fails
THE SYSTEM SHALL keep the in-memory entry visible
AND SHALL NOT attempt delivery until the full visible outbox is successfully persisted
AND SHALL retry persistence on later delivery triggers

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
`message_id = localId`, reconciliation against either an exact authoritative
id or the server's conversation-scoped canonical id, union-without-duplicates
rendering), with two platform deviations:

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

WHEN a message update arrives before its identity-bearing message event
THE SYSTEM SHALL retain the update by message id
AND apply it when the message arrives

WHEN a conversation hard-delete event arrives
THE SYSTEM SHALL remove its transcript, snapshot, outbox, and list entry
AND SHALL disable further interaction with the deleted conversation

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

WHEN the server presents a certificate that passes standard trust evaluation
AND no pin exists for that host and port
THE SYSTEM SHALL accept it without creating a pin

WHEN a pin exists for the host and port
THE SYSTEM SHALL compare the presented leaf certificate before accepting it
regardless of whether standard trust evaluation succeeds

WHEN the server presents a self-signed certificate, the user has enabled the
trust toggle, and no certificate is pinned for that host and port
THE SYSTEM SHALL accept the certificate
AND pin its SHA-256 fingerprint (trust on first use)

WHEN a pinned host presents a certificate whose fingerprint differs from
the pin
THE SYSTEM SHALL reject the connection
AND surface the mismatch with an explicit re-trust affordance that forgets
the pin (the next connection then re-pins)

WHEN the user signs out
THE SYSTEM SHALL forget the pin along with all other per-server state

**Rationale:** Matches the non-browser client auth scheme (the phoenix-auth
cookie is a browser session token, not a client credential) and the
self-signed TLS posture of typical Phoenix deployments. Blanket acceptance
of any certificate would let a MITM on a hostile network capture the Bearer
password; trust-on-first-use pinning closes that after first contact with
zero configuration.

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
THE SYSTEM SHALL render user text, agent text blocks, tool-use cards, and
tool result cards
AND stream in-flight agent text from token events
AND fall back to a compact JSON rendering for unrecognized content shapes
rather than omitting them

WHEN rendering a tool invocation or result
THE SYSTEM SHALL dispatch on the tool name to a native renderer when one
exists
AND join a tool result to its invoking tool_use block by `tool_use_id` to
determine the tool name
AND fall back to a generic card (name + input summary for invocations;
status line + expandable output for results) for tools without a native
renderer or when the join fails

WHEN rendering a `bash` invocation
THE SYSTEM SHALL show the command (preferring the server-cleaned `display`
string) in monospace, and render non-run ops as `op handle`

WHEN rendering a `bash` result
THE SYSTEM SHALL parse the typed response envelope (success shapes tagged
by `status`, error shapes tagged by `error` — see `specs/bash/`)
AND show a one-glance outcome header (status, exit code or signal,
duration, handle for in-flight ops) colored by outcome
AND for tombstones use `final_cause` as the terminal outcome even when
`signal_number` is absent
AND show the output tail collapsed, with the full output expandable and a
truncation notice when the server truncated
AND degrade to the generic result card when the payload is not a parseable
envelope

WHEN rendering a `think` invocation
THE SYSTEM SHALL render the recorded thoughts as a quiet quote block
AND suppress the paired tool result unless it is an error (the success
payload is fixed boilerplate addressed to the LLM, not the user)

**Rationale:** Readable transcripts with progressive disclosure; unknown
shapes must degrade visibly, not vanish (omission is data loss). Native
renderers are per-tool and additive; the generic cards are the structural
floor that makes every tool legible before it earns a dedicated view.

---

### REQ-IOS-011: Typed Conversation State Rendering

WHEN a conversation state arrives (init snapshot, state_change event, or
cached conversation)
THE SYSTEM SHALL decode it into a typed state: recognized variants carry
the fields the UI consumes; a recognized envelope with an unhandled
variant degrades to a labeled catch-all; an unparseable payload degrades
to unknown

WHEN rendering the conversation, the typed state SHALL drive a state
detail area between transcript and composer:
- in-flight states render inline working detail (current tool name plus
  completed/queued counts for tool execution; retry attempt for LLM
  requests; sub-agent progress counts)
- states requiring the user render a prominent needs-action card (question
  asked, task plan awaiting approval, context exhausted)
- the error state renders an error card carrying the message and the
  dismiss action
- any unhandled state whose presentation mode is error renders a visible
  error card rather than disappearing

WHEN deciding whether the agent is busy
THE SYSTEM SHALL use the server's presentation_mode, not re-derive it from
the typed state

**Rationale:** The state machine visualization is the mobile UI's primary
feedback mechanism (the REQ-API-011 rationale); string-matching state
names at each usage site drifts. The decode-with-fallback shape matches
the SSE event and tool-renderer patterns so a newer server degrades
rendering instead of breaking it.

---

### REQ-IOS-012: Action Delivery Policy

WHEN a user-initiated conversation operation is defined
THE SYSTEM SHALL declare its delivery policy in the action's type:
- outboxed: persisted locally before any network I/O, idempotency-keyed,
  auto-retried (chat messages)
- online-only: requires a live server answer because it reads or
  transitions live server state (cancel, dismiss-error, archive)

WHEN an online-only action is invoked while offline
THE SYSTEM SHALL disable the control or fail immediately with an
explanation
AND SHALL NOT queue the action for later replay

WHEN an online-only action is rejected by the server (e.g. dismissing a
non-resumable error)
THE SYSTEM SHALL surface the server's explanation

WHEN the conversation is in a state that rejects ordinary chat
THE SYSTEM SHALL disable the composer
AND SHALL continue allowing chat in working states where the server accepts
the message as steering

**Rationale:** Queuing an action against live server state fabricates a
stale intent — an archive or cancel replayed minutes later can destroy
work the user did in between. Only idempotency-keyed sends are safe to
defer; the type forces each new action to make that choice explicitly.

---

### REQ-IOS-013: Task Approval

WHEN a conversation is awaiting task approval
THE SYSTEM SHALL render the proposed task's title, priority, and plan
(plan collapsed with an expand affordance)
AND offer approve, reject (with confirmation), and free-text
request-changes resolutions

WHEN a resolution is submitted
THE SYSTEM SHALL send it as an online-only action (REQ-IOS-012)
AND rely on the server's resulting state change to clear the card rather
than optimistic local state
SO THAT a decision made concurrently from another client wins cleanly and
this client simply observes the state move on

WHEN the device is offline or a resolution is in flight
THE SYSTEM SHALL disable the resolution controls
AND, when offline, state that approval is never queued

**Rationale:** Plan approval is the highest-value blocking decision to
make away from the desk. The no-optimistic-state rule matters because
approval is multi-client: the server 400s a decision on an
already-decided plan, which surfaces as an explanatory error instead of a
silent double-apply.

---

### REQ-IOS-014: Versioned Persistence

WHEN a durable store (outbox, conversation snapshot, conversation list)
is written
THE SYSTEM SHALL wrap the payload in an envelope carrying a schema version

WHEN loading a store whose version matches
THE SYSTEM SHALL decode it directly

WHEN loading an older version
THE SYSTEM SHALL route through that store's migration hook

WHEN loading a NEWER version (downgraded app)
THE SYSTEM SHALL treat the file as absent rather than misparse it
AND SHALL NOT delete it
AND SHALL refuse to overwrite it until the user upgrades or explicitly
clears the store

WHEN loading a pre-envelope legacy file
THE SYSTEM SHALL decode the bare payload as version zero

Changing any persisted struct requires either a version bump plus a
migration branch, or a field-level note that the change is
additive-optional (old files decode it as nil/default).

**Rationale:** Without an envelope, any shape change makes old files
undecodable and lenient decoding silently discards them — for the outbox
that is queued-message loss, the exact failure the app exists to prevent.
The version makes "old data" distinguishable from "corrupt data" and
makes forgetting a migration a reviewable event instead of a silent wipe.

---

### REQ-IOS-015: Image Attachments

WHEN a message or tool result carries typed image payloads
THE SYSTEM SHALL render them inline (tool-result images render even while
the output is collapsed)
AND an undecodable image SHALL render a labeled placeholder, never nothing

WHEN the user attaches photos to a message
THE SYSTEM SHALL downscale to a bounded long edge and recompress before
staging
AND queue them through the same outbox entry as the text (same
durability, same idempotent delivery)
AND surface any photo that failed to load rather than dropping it from
the send

WHEN a queued entry carries images
THE SYSTEM SHALL indicate the attachment count on the optimistic bubble

**Rationale:** Images flow through one typed path in both directions
(mirrors the web's single-source-of-truth rule for ToolContent.images).
Client-side downscaling keeps multi-megapixel photos from bloating outbox
files and chat POSTs; the visible-failure rules are the transcript-wide
omission-is-data-loss principle applied to media.

---

### REQ-IOS-016: Question Answering

WHEN a conversation is awaiting a user response
THE SYSTEM SHALL render each question with its header, text, and options
(with descriptions), honoring single- versus multi-select semantics
AND offer a free-text "Other" answer per question
AND offer dismissal (with confirmation) as the no-answer resolution

WHEN the question payload is empty or contains no decodable questions
THE SYSTEM SHALL still offer the online-only dismissal action

WHEN encoding answers
THE SYSTEM SHALL key them by question text, with a single-select answer
being the chosen option label (or the trimmed Other text) and a
multi-select answer joining chosen labels in declared option order with
", ", appending trimmed Other text
AND submission SHALL be disabled until every question has an answer
AND a selection for a label absent from the current options SHALL not
count as an answer

WHEN a response is submitted
THE SYSTEM SHALL follow the interactive-resolution rules of REQ-IOS-013:
online-only, no optimistic state (the server's state change clears the
card; concurrent resolution from another client surfaces as the server's
conflict), controls disabled while offline or in flight, and drafts
preserved until success

**Rationale:** A stalled agent is worth nothing until answered; this is
the highest-value blocking state to resolve away from the desk. The
encoding contract mirrors the web QuestionPanel so the server observes
identical answer shapes from every client.

---

### REQ-IOS-017: Fleet Coordinator Access

WHEN the user opens the Coordinator
THE SYSTEM SHALL get-or-create it via the global coordinator endpoint and
navigate to it as an ordinary conversation (standard transcript, caching,
outbox, and actions apply unchanged)

WHEN offline with a previously opened Coordinator
THE SYSTEM SHALL open its cached transcript by the remembered id, with
new questions queueing through the outbox
AND first-time opening SHALL require connectivity

WHEN the Coordinator appears in the conversation list
THE SYSTEM SHALL badge it distinctly

WHEN the user opens list actions for the Coordinator
THE SYSTEM SHALL NOT offer archive

The remembered Coordinator id is per-server state and SHALL be cleared on
sign-out.

**Rationale:** The Coordinator is Phoenix's most mobile-shaped surface —
one conversation that answers questions about the whole fleet. Because
the server models it as an ordinary conversation, the client adds only an
entry point; every offline guarantee is inherited rather than rebuilt.

---

### REQ-IOS-018: Advisory Background Nudges

WHEN the user enables nudges (behind notification authorization)
THE SYSTEM SHALL schedule opportunistic background refreshes
AND on each run fetch the conversation list, fire one local notification
per conversation that newly entered needs-action or error, or completed a
working turn, and refresh the cached list

WHEN diffing against the last-seen snapshot
THE SYSTEM SHALL never notify for a conversation absent from the snapshot
(first sight seeds silently)
AND SHALL re-seed silently on every foreground refresh, so the user is
never nudged about state they already saw

WHEN a notification is tapped
THE SYSTEM SHALL navigate to that conversation (cold launch included)

WHILE the app is foregrounded
THE SYSTEM SHALL suppress nudge banners

Missed, delayed, or skipped background runs SHALL NOT affect correctness
— the tier is advisory only, and no product behavior may come to depend
on a run occurring.

**Rationale:** Opportunistic refresh can provide useful advisory awareness
without becoming part of the correctness model. iOS controls the refresh
cadence (≥15 min, best-effort), so every nudge is explicitly non-authoritative.
