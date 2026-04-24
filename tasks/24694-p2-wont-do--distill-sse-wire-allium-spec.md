---
created: 2026-04-23
priority: p2
status: wont-do
artifact: pending
---

# distill-sse-wire-allium-spec

## Plan

# Distill `sse_wire.allium` — SSE Protocol Contract

## Context

Task **02680** asks for a formal Allium spec of the SSE wire protocol: all 9 event types, their ordering constraints, and a named `PersistBeforeBroadcast` invariant. The spec must pass `allium check` with 0 errors and 0 warnings (existing specs have validation issues; this new file must be clean).

### Source evidence gathered

| Source | Key facts |
|--------|-----------|
| `src/api/wire.rs` | `SseWireEvent` enum — 9 variants, all carry `sequence_id: i64`, `#[serde(tag = "type")]` |
| `src/runtime.rs` | `SseBroadcaster` owns monotonic counter; `send_message()` called **after** DB write; `send_seq()` for ephemeral events; capacity 4096 |
| `src/api/sse.rs` | `BroadcastStreamRecvError::Lagged` → close stream + warn; client reconnects; init resyncs from DB |
| `ui/src/hooks/useConnection.ts` | `parseEvent()` validates schemas; all 9 event types handled |
| `tasks/02680-*` | Scope + invariants listed; cross-refs to 02679, 02681, conversation_atom.allium |
| `specs/conversation_atom/conversation_atom.allium` | Template: self-contained, no imports, value types + entities + rules + invariants |
| Allium language ref | Temporal ordering invariants are **not expressible** as structural invariants — must use prose. Structural invariants use `for x in Entities: expression`. Entity collections: natural plural of entity name. |

### Entity model

The key design insight: to make `PersistBeforeBroadcast` a **named structural invariant** (not prose), model both `PersistedMessage` and a `StreamMessage` join entity internally:

- `PersistedMessage` — created by `MessageCommittedToDb` rule (external stimulus → entity); enables `PersistedMessage.created` trigger for chaining
- `StreamMessage` — join entity: one per (stream, message-event) delivery; `PersistBeforeBroadcast` asserts every `StreamMessage` has a backing `PersistedMessage`
- `SseStream` — the connection; `status: open | closed`, `init_sent: Boolean`, `last_delivered_seq: Integer`; `transitions status { open -> closed; terminal: closed }`

## Plan

### 1. Read reference material (in-task)
- `.agents/skills/allium/references/language-reference.md` (full)
- `specs/conversation_atom/conversation_atom.allium` (template)
- Source files already summarized above (re-read if needed)

### 2. Write `specs/sse_wire/sse_wire.allium`

Create directory `specs/sse_wire/` and write the spec. Structure:

```
-- allium: 3
-- sse_wire.allium
-- Scope: SSE wire protocol — server-side obligations to clients
-- Includes: all 9 event types, ordering constraints, persist-before-broadcast
-- Excludes: client reducer (conversation_atom.allium), reconnect machine (02681)
-- Self-contained: no `use` imports

[Value Types]
  InitSnapshot        { last_sequence_id: Integer, ... opaque }
  UserFacingError     { message: String, ... opaque }
  ConversationMetadataUpdate { cwd: String?, ... opaque }

[Entities]
  PersistedMessage    { conversation_id, message_id, sequence_id }
  StreamMessage       { stream: SseStream, message_id, sequence_id }  -- join entity
  SseStream           { conversation_id, status: open|closed, init_sent, last_delivered_seq
                        stream_messages: StreamMessage with stream = this
                        transitions status { open -> closed; terminal: closed } }

[Config]
  broadcast_capacity: Integer = 4096

[Rules — 11 total]
  StreamOpened                         — ClientSubscribes → SseStream.created(init_sent=true, last_delivered_seq=snapshot.last_sequence_id)
  MessageCommittedToDb                 — external stimulus → PersistedMessage.created
  MessageBroadcast                     — PersistedMessage.created → StreamMessage.created per open stream + advance last_delivered_seq
  MessageUpdatedBroadcast              — MessageFieldsMutated → advance last_delivered_seq
  StateChangeBroadcast                 — ConversationStateTransitioned → advance last_delivered_seq
  TokenBroadcast                       — LlmTokenReceived → advance last_delivered_seq (no StreamMessage — ephemeral)
  AgentDoneBroadcast                   — LlmTurnCompleted → advance last_delivered_seq; @guidance: fires after MessageBroadcast for the turn
  ConversationBecameTerminalBroadcast  — ConversationEnteredTerminalState → advance last_delivered_seq
  ConversationUpdateBroadcast          — ConversationMetadataChanged → advance last_delivered_seq
  ErrorBroadcast                       — UserFacingErrorOccurred → advance last_delivered_seq
  LagCloseStream                       — BroadcastLagDetected → stream.status = closed; @guidance: client reconnects, fresh init resyncs from DB

[Invariants — 3 structural]
  PersistBeforeBroadcast
    -- Named, load-bearing: every StreamMessage has a backing PersistedMessage.
    -- This is the emit-vs-persist race (task 02679): if broadcast fires before
    -- DB commit, a StreamMessage would exist with no PersistedMessage.
    for sm in StreamMessages:
        exists PersistedMessage{message_id: sm.message_id, conversation_id: sm.stream.conversation_id}

  InitAlwaysFirst
    -- Before init_sent, no events have been delivered.
    for stream in SseStreams:
        not stream.init_sent implies stream.last_delivered_seq = 0

  SequencesNonNegative
    for stream in SseStreams:
        stream.last_delivered_seq >= 0
```

**Temporal-ordering properties** (AgentDoneTerminatesTokens, MessagePrecedesUpdates, LagCloseRecoverability) are expressed via rule `requires`/`@guidance` blocks — they are temporal ordering assertions and are not expressible as Allium structural invariants per the language spec.

### 3. Run `allium check` and fix

```bash
allium check specs/sse_wire/sse_wire.allium
```

Iterate until 0 errors, 0 warnings. Common issues to anticipate:
- Inline enum type identity (if two rules compare inline enum values from different entities — use named enum `StreamStatus` if needed)
- `exists Entity{field: expr}` syntax in invariants (may need `let` binding form)
- `for x in Set<String>` iteration (may need to switch to relationship-based iteration if `Set<String>` iteration is unsupported)
- `where` clause field disambiguation if trigger parameter names collide with entity field names (use `conv_id` vs `conversation_id`)
- Missing `status: open` guard on ensures that mutate `last_delivered_seq` (closed streams must not be mutated)

### 4. Mark task done and commit

```bash
taskmd status 02680 done
git add specs/sse_wire/sse_wire.allium tasks/02680-*.md
git commit -m "spec(sse_wire): distill SSE wire protocol as allium spec"
```

## Acceptance criteria

- `specs/sse_wire/sse_wire.allium` exists
- `allium check specs/sse_wire/sse_wire.allium` → 0 errors, 0 warnings
- All 9 SSE event types have a rule
- `PersistBeforeBroadcast` is a named structural invariant (not prose)
- `InitAlwaysFirst` is a named structural invariant
- Lag semantics captured in `LagCloseStream` rule with `@guidance`
- Self-contained (no `use` imports)
- Task 02680 marked done, change committed


## Progress

