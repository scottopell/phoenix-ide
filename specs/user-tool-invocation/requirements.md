# User Tool Invocation — Requirements

## User Need

A user wants to invoke Phoenix's worker tools directly — running a tool as
themselves, without an agent turn — from the conversation composer, using a
lightweight inline syntax. The results are recorded as **user-originated tool
rounds** that appear in conversation history and LLM context attributed to the
user, so a later agent turn inherits an accurate picture of what the human did.

This spec is the umbrella for the user-originated tool round model. The inline
terminal (`specs/inline-terminal`) is its first concrete consumer, specialized
to `bash` behind the `!` sigil and an interactive PTY; this spec generalizes the
model to the other worker tools and defines what they share.

## Terminology

- **User-originated tool round** — a tool-use block plus its tool-result,
  committed to conversation history attributed to the user rather than the
  agent. Shaped like an agent tool round so the LLM history builder reads it on
  the same path.
- **Worker tool** — a tool that performs work on the conversation's behalf
  (`bash`, `patch`, `keyword_search`, `read_image`, `think`), as opposed to an
  agent-control tool that drives the agent lifecycle (`spawn_agents`,
  `submit_result`, `propose_task`, `ask_user_question`).

## Requirements

### REQ-UTI-001 — Composer syntax for direct tool invocation

A sigil at the start of the composer denotes a direct user tool invocation
(`$tool …` / `T.tool …`) carrying the tool's arguments. The syntax is distinct
from the `@`-reference and `/`-skill expansion (`specs/inline-references`,
`specs/skills`), which rewrite a prose message that is still submitted as a
user message and triggers the agent. A user tool invocation submits no prose
message and triggers no agent turn.

### REQ-UTI-002 — Parse-time validation against the tool registry

The invocation is validated against the registered worker tools when the
composer submits it: an unknown tool name, a tool not user-invokable
(REQ-UTI-003), or arguments that fail to parse into the tool's typed input are
rejected synchronously with an actionable error. A typo never becomes a
malformed tool that executes — validation happens before any effect runs.

### REQ-UTI-003 — Worker tools only

Only worker tools are user-invokable: `bash`, `patch`, `keyword_search`,
`read_image`, `think`. Agent-control tools (`spawn_agents`, `submit_result`,
`propose_task`, `ask_user_question`) are lifecycle plumbing with no meaning when
invoked by the user and are not exposed to this syntax.

### REQ-UTI-004 — Track B shared history

Each invocation commits a user-originated tool round — a tool-use block (the
tool name and typed input) plus its tool-result — to conversation history,
delivered to the LLM as a tool-use/tool-result pair. A later agent turn reads it
exactly as it reads its own tool calls (the shared-history model), so manual
work and agent work compose into one coherent record.

### REQ-UTI-005 — User origin is the single source of truth for attribution

Every user-originated tool round carries one origin marker: user. The role the
round takes in LLM history (prior tool activity) and the attribution shown in
the conversation UI (a user-initiated invocation) are both derived from that
single marker, never stored independently. Provenance keeps a user-run tool
honestly distinguishable from an agent-issued one.

### REQ-UTI-006 — Idle-gated, no agent turn

A user tool invocation is accepted only when the conversation is idle; while one
runs the conversation is busy and the agent does not run; completing it returns
the conversation to idle without issuing an LLM request. The user advances the
conversation without an agent turn.

### REQ-UTI-007 — The inline terminal is the `bash` specialization

The inline terminal (`specs/inline-terminal`) realizes this model for `bash`:
the `!` sigil opens an interactive PTY whose OSC-133-delimited commands commit as
user-originated `bash`-run rounds. It is a richer surface than a single `$bash …`
invocation (full interactivity, per-command commit), and this spec does not
redefine it. A non-interactive `$bash …` invocation and the inline terminal
share the same round shape and attribution.

## Out of Scope

These are settled per consumer or deferred to implementation, not fixed here:

- The exact sigil and argument grammar (`$tool` vs `T.tool`; positional vs
  JSON vs shell-style arguments). Bash may keep a bare-string shorthand
  (`$bash <text>` → `{command: text}`); other tools carry structured input.
- Per-tool composer affordances (completion, argument hints).
- The interactive-PTY specialization for `bash`, which is the inline terminal's
  own spec.
- The behavioural model (states, transitions, invariants) — added as an Allium
  spec when the general syntax is implemented, factoring out the shared
  user-origin round machinery the inline terminal realizes for `bash`.
