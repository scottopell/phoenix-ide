# User Tool Invocation — Executive Summary

## What It Is

A composer syntax for running eligible Phoenix tools directly as the user —
`$tool …` / `T.tool …` — without provoking an agent turn. Each invocation is
recorded as a **user-originated tool round** (a tool-use/tool-result pair
attributed to the user) that a later agent turn reads exactly as it reads its
own tool calls. The user can do a unit of work themselves and have it become a
first-class, recorded part of the conversation rather than an out-of-band side
channel.

This is the umbrella for the user-originated tool round model. The inline
terminal (`specs/inline-terminal`) is its first concrete consumer, specialized
to `bash` behind `!` and an interactive PTY.

## Why It Exists

The agent and the user share one conversation and one working directory.
Sometimes the user wants to drive directly — run a formatter, search the code,
patch a file — without handing the turn to the model, while keeping the model's
eventual view accurate. User tool invocation makes "the human ran a tool" a
recorded, attributable event in the same history the agent reads.

## Scope

**Included:**
- A composer sigil for direct tool invocation, distinct from `@`/`/` prose
  expansion
- Parse-time validation against the registry (unknown tool / not-eligible / bad
  arguments rejected before any effect runs)
- Eligibility by criterion, not a fixed list: meaningful authorship +
  self-service (returns to idle) + not dominated by a native affordance. The
  surviving members are `bash` (the inline terminal) and tools with no shell
  equivalent, chiefly MCP and project-registered integrations
- Track B shared history: each invocation is a user-originated tool round
  visible to a later agent turn
- User origin as the single source of truth for attribution
- Idle-gating with no agent turn

**Excluded / deferred:**
- User-as-director tools (`spawn_agents`, `propose_task`) — meaningful but not
  self-service (they launch agent activity), deferred until concrete use cases
  emerge
- Dominated tools (`patch`, `keyword_search`, `read_image`, `read_file`) — the
  inline terminal already serves their journey (`!nvim`, `!rg`, `!cat`)
- LLM-internal / inter-agent-protocol tools (`think`, `submit_result`,
  `ask_user_question`) — no user journey
- The exact sigil and argument grammar (a design choice, possibly per-tool)
- Per-tool composer affordances (completion, hints)
- The interactive-PTY `bash` specialization — owned by `specs/inline-terminal`
- The behavioural (Allium) model — added when the general syntax is built

## Relationship to the Inline Terminal

The inline terminal realizes this model for `bash`: `!` opens an interactive PTY
whose OSC-133-delimited commands commit as user-originated `bash`-run rounds. It
shares this spec's round shape, attribution, and idle-gating, and adds
interactivity and per-command commit on top. When the general syntax is
implemented, the shared user-origin round machinery (the attribution
discriminator, the Track B commit path) is the common core both surfaces use;
this spec is where that core is defined, and the inline terminal is the proof
that the model carries a real consumer.

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| History model | Shared (Track B) | A user-run tool reads back to the agent exactly like an agent tool round; manual and agent work compose into one record |
| Attribution | Single `origin` discriminator | LLM-history role and UI attribution both derived; no parallel representations |
| Tool scope | Eligibility criterion, not a list: meaningful + self-service + not dominated | Survivors are `bash` and shell-less integrations (MCP/custom); dominated tools defer to the terminal, director tools are deferred |
| Validation | Parse-time, against the registry | A typo is a synchronous rejection, never a malformed tool that executes |
| Conversation state | Idle-gated, no agent turn | The user advances the conversation without invoking the model |
| `bash` surface | The inline terminal | An interactive PTY is a richer fit for `bash` than a one-shot string; same round shape |

## Status Summary

The model is defined here and specified for `bash` by the inline terminal
(`specs/inline-terminal`). Neither this spec nor the inline terminal is
implemented yet; the general multi-tool syntax is specified at the requirements
level only.

| Requirement | Status |
|---|---|
| REQ-UTI-001: Composer syntax for direct tool invocation | Planned |
| REQ-UTI-002: Parse-time validation against the registry | Planned |
| REQ-UTI-003: Eligible tools (meaningful, self-service, not dominated) | Planned |
| REQ-UTI-004: Track B shared history | Specified for `bash` (inline terminal) |
| REQ-UTI-005: User origin single source of truth | Specified for `bash` (inline terminal) |
| REQ-UTI-006: Idle-gated, no agent turn | Specified for `bash` (inline terminal) |
| REQ-UTI-007: Inline terminal is the `bash` specialization | Specified (`specs/inline-terminal`) |
