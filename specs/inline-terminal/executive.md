# Inline Terminal — Executive Summary

## What It Is

A real, PTY-backed terminal hosted inside the conversation composer, opened by
typing `!` as the first character of the message input. The user runs shell
commands themselves — full line editing, completion, colour, interactive
programs — and each command is recorded into the conversation as a `bash` tool
round the agent can later read. Issuing those commands never asks the model for
a response: the user drives the conversation forward without an agent turn.

It reuses the entire terminal stack from `specs/terminal` (PTY, binary
WebSocket, xterm.js, vt100 parser, OSC 133 command tracking) and adds the bridge
from a live terminal's commands to durable, user-attributed history.

## Why It Exists

A user sometimes wants to do the work themselves — run a build, poke at the
filesystem, reproduce a failure — without provoking the agent, while keeping the
agent's eventual view of the conversation accurate. The inline terminal makes
"the human took the wheel for a few commands" a first-class, recorded part of the
conversation rather than an out-of-band side channel the agent never sees.

## Scope

**Included:**
- The `!` composer trigger and inline PTY session lifecycle (open/close)
- Conversation gating: open only when idle; the conversation is busy while live;
  closing returns to idle with no agent turn
- Per-command commit: each OSC-133-delimited command becomes one user-originated
  `bash`-`run` tool round in shared history (visible to a later agent turn)
- The interrupted-round rule: a started-but-uncompleted command is committed
  (never dropped) when its bracket is superseded by the next command or by
  session close
- User origin as the single source of truth for round attribution

**Excluded / deferred:**
- Cursor-addressing program output (`vim`, `htop`, progress bars) — alt-screen
  programs want an empty result, in-place rewriting wants vt100-resolved text; a
  shared `specs/terminal` `CommandTracker` concern, and the result type leaves
  room for either
- Backgrounded command operations (the `bash` wait/kill/handle model)
- Sharing one session with the per-WorkScope panel terminal
- The general `$tool` / `T.tool` user-invocation syntax for other eligible tools (`specs/user-tool-invocation`)

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Open trigger | `!` as first composer char | Distinct from `@`/`/` prose expansion; switches the composer to terminal mode rather than submitting a message |
| History model | Shared (Track B) | Inline rounds shaped exactly like agent `bash` rounds so a later agent turn reads them with no special case |
| Round shape | `bash` tool-use + tool-result, `op: run` | Reuses the agent tool-round message path; no new `MessageContent` variant |
| Command granularity | Per OSC 133 command | The `CommandTracker` already decomposes a session into discrete `CommandRecord`s — the natural commit unit |
| Started-but-uncompleted command | Interrupted round on supersession | Every `C` resolves to exactly one round; user activity never silently disappears |
| Conversation state while live | Busy (`inline_terminal`), never requests LLM | Mutual exclusion with agent tool rounds prevents two writers interleaving into history |
| Session vs panel terminal | Separate, short-lived | Preserves `specs/terminal` "one panel terminal per WorkScope" |
| Attribution | Single `origin` discriminator | LLM-history role and UI attribution are both derived; no parallel representations (mirrors `MessageContent::Skill`) |
| Result content type | Typed `ToolResult` content, not bare `String` | Leaves room for cursor-addressing-program resolution (empty for alt-screen, vt100-resolved for in-place rewriting) without migration |
| Deny-gate | Not applied | Server-hosted shell (server's Unix user), ungated like the panel terminal; honesty comes from `origin: user` provenance |

## The Central Invariant

Every command that starts in an inline session resolves to **exactly one**
committed round — `completed` (its OSC 133 `D` arrived) or `interrupted` (its
bracket was superseded by the next command or by session close). There is no
dangling state and no silent loss. This totality is what makes the shared-history
guarantee trustworthy: the agent's later view of "what the human did" has no
holes, even when the human killed a command or closed the session mid-run.

## Status Summary

The feature is specified but not yet implemented.

| Requirement | Status |
|---|---|
| REQ-IT-001: `!` opens an inline terminal in the composer | Planned |
| REQ-IT-002: Opening gated to an idle conversation; busy while live | Planned |
| REQ-IT-003: Each command committed as a user-originated tool round | Planned |
| REQ-IT-004: Inline rounds use the `bash` run operation only | Planned |
| REQ-IT-005: A started command is never dropped (interrupted round) | Planned |
| REQ-IT-006: Closing the session never triggers an agent turn | Planned |
| REQ-IT-007: User origin is the single source of truth for attribution | Planned |
| REQ-IT-008: Inline commands run un-gated, with honest provenance | Planned |
