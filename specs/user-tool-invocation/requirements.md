# User Tool Invocation — Requirements

## User Need

A user wants to invoke certain of Phoenix's tools directly — running a tool as
themselves, without an agent turn — from the conversation composer, using a
lightweight inline syntax. The results are recorded as **user-originated tool
rounds** that appear in conversation history and LLM context attributed to the
user, so a later agent turn inherits an accurate picture of what the human did.

This spec is the umbrella for the user-originated tool round model. The inline
terminal (`specs/inline-terminal`) is its first concrete consumer, specialized
to `bash` behind the `!` sigil and an interactive PTY. Beyond `bash`, the
members that remain after the eligibility criterion (REQ-UTI-003) are tools with
no shell equivalent — chiefly MCP and project-registered integrations — since
the inline terminal already serves anything a shell can do.

## Terminology

- **User-originated tool round** — a tool-use block plus its tool-result,
  committed to conversation history attributed to the user rather than the
  agent. Shaped like an agent tool round so the LLM history builder reads it on
  the same path.
- **Self-service tool** — a tool whose invocation produces a result and leaves
  the conversation idle, with no agent turn launched. Eligibility for user
  invocation (REQ-UTI-003) is scoped to self-service tools; tools that *launch*
  agent activity (user-as-director) are deferred.

## Requirements

### REQ-UTI-001 — Composer syntax for direct tool invocation

A sigil at the start of the composer denotes a direct user tool invocation
(`$tool …` / `T.tool …`) carrying the tool's arguments. The syntax is distinct
from the `@`-reference and `/`-skill expansion (`specs/inline-references`,
`specs/skills`), which rewrite a prose message that is still submitted as a
user message and triggers the agent. A user tool invocation submits no prose
message and triggers no agent turn.

### REQ-UTI-002 — Parse-time validation against the tool registry

The invocation is validated against the registered tools when the composer
submits it: an unknown tool name, a tool not eligible for user invocation
(REQ-UTI-003), or arguments that fail to parse into the tool's typed input are
rejected synchronously with an actionable error. A typo never becomes a
malformed tool that executes — validation happens before any effect runs.

### REQ-UTI-003 — Eligible tools: meaningful, self-service, not dominated

A tool is exposed to direct user invocation only when all three hold:

1. **Authorship is meaningful** — the call represents an action a human would
   plausibly want to perform themselves, not LLM-internal cognition (`think`)
   or an inter-agent protocol message (`submit_result`, `submit_error`,
   `ask_user_question`, `commission_review`).
2. **Self-service** — the invocation produces a result and returns the
   conversation to idle without launching further agent activity (REQ-UTI-006).
   Tools that start agent work are user-as-director actions, deferred (see Out
   of Scope).
3. **Not dominated** — no native affordance gives the user the *same effect*
   more directly. The inline terminal dominates every tool with a shell
   equivalent that also records to shared history: `patch` (use `!nvim`),
   `keyword_search` / `read_file` (use `!rg` / `!cat`). Domination is about
   effect, not surface: a viewer is *not* a substitute for `read_image`, which
   carries the image bytes through the typed image channel into LLM context —
   viewing shows the pixels to the human only. `read_image` is therefore
   eligible: a user can self-service a screenshot or generated image into shared
   history without an agent turn.

The members that survive are `bash` — realized by the inline terminal
(REQ-UTI-007) — `read_image`, and self-service tools with no shell equivalent,
chiefly MCP and project-registered integrations. The eligible set is defined by
this criterion, not a fixed list, so a newly registered tool is evaluated against
it rather than hardcoded.

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
conversation without an agent turn. This return-to-idle property is what scopes
eligibility to self-service tools (REQ-UTI-003): a tool that launches agent
activity instead of returning to idle is a user-as-director action, out of scope
here.

### REQ-UTI-007 — The inline terminal is the `bash` specialization

The inline terminal (`specs/inline-terminal`) realizes this model for `bash`:
the `!` sigil opens an interactive PTY whose OSC-133-delimited commands commit as
user-originated `bash`-run rounds. It is a richer surface than a single `$bash …`
invocation (full interactivity, per-command commit), and this spec does not
redefine it. A non-interactive `$bash …` invocation and the inline terminal
share the same round shape and attribution.

## Out of Scope

These are settled per consumer or deferred to implementation, not fixed here:

- **User-as-director tools.** Tools that assert an orchestration decision the
  agent would otherwise make — `spawn_agents` (the user's own fan-out strategy),
  `propose_task` (the user's own task definition) — are meaningful for direct
  invocation but are *not* self-service: they launch agent activity rather than
  returning to idle, and a user-authored `propose_task` collapses the
  propose/approve loop into author-and-self-approve, needing a user-authority
  variant rather than verbatim reuse. They remain a candidate extension of this
  model, deferred until concrete use cases pin down their interaction and state
  semantics.
- The exact sigil and argument grammar (`$tool` vs `T.tool`; positional vs
  JSON vs shell-style arguments). Bash may keep a bare-string shorthand
  (`$bash <text>` → `{op: run, cmd: text}`, matching `BashToolInput`); other
  tools carry structured input.
- Per-tool composer affordances (completion, argument hints).
- The interactive-PTY specialization for `bash`, which is the inline terminal's
  own spec.
- The behavioural model (states, transitions, invariants) — added as an Allium
  spec when the general syntax is implemented, factoring out the shared
  user-origin round machinery the inline terminal realizes for `bash`.
