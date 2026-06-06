# Named Agents

## User Story

As a user, I need to define named sub-agents — personas with their own
instructions, default model, and default mode — as files in my project, so the
LLM can delegate to a purpose-built agent by name without me re-describing the
persona every conversation.

As an LLM agent, I need discovered named agents to appear as a typed choice on
the spawn tool, so I can reliably select the right persona for a delegated task
instead of free-typing an ad-hoc prompt.

> Named agents extend the existing sub-agent machinery (see
> [`../subagents/`](../subagents/requirements.md)): an agent definition supplies
> the *persona, default model, and default mode* for a spawned sub-agent. The
> spawn lifecycle, isolation, fan-in, cancellation, and timeout rules are
> unchanged and remain normative in [`subagents.allium`](../subagents/subagents.allium)
> and [`bedrock.allium`](../bedrock/bedrock.allium). Detailed agent behaviour —
> discovery, frontmatter, schema-enum injection, resolution precedence, persona
> composition — is normative in [`agents.allium`](./agents.allium).

## Requirements

### REQ-AG-001: Agent Definition Discovery

THE SYSTEM SHALL discover agent definitions by scanning `.claude/agents/` and
`.agents/agents/` directories at each level from the conversation's working
directory up to the filesystem root

THE SYSTEM SHALL also scan immediate child directories of the working directory
for agent definitions (handling the "projects directory" case)

THE SYSTEM SHALL also scan `$HOME/.claude/agents/` and `$HOME/.agents/agents/`
when `$HOME` is not an ancestor of the working directory

WHEN the same agent name appears at multiple levels
THE SYSTEM SHALL use the one closest to the working directory (more specific
overrides parent)

WHEN two paths resolve to the same file (via symlinks)
THE SYSTEM SHALL count them as one agent (first discovered wins)

WHEN two different files have identical content
THE SYSTEM SHALL count them as one agent (content-hash dedup)

**Rationale:** Agents are contextual in exactly the way skills are — a
project-level `reviewer` agent overrides a user-level `reviewer` because it is
more specific. Mirroring the skill discovery walk (REQ-SK-006) keeps a single
mental model and lets the implementation reuse the proven walk-up and dedup
logic.

---

### REQ-AG-002: Agent Definition Format

THE SYSTEM SHALL represent each agent as a single Markdown file in an agents
directory, where the file's YAML frontmatter carries the agent's metadata and
the file body is the agent's persona instructions

THE SYSTEM SHALL require the frontmatter fields `name` and `description`

THE SYSTEM SHALL accept the optional frontmatter fields `model` (a default
model id) and `mode` (`explore` or `work`)

WHEN an agents directory contains a file whose frontmatter is missing a
required field
THE SYSTEM SHALL skip that file without registering an agent and without
aborting discovery of the others

**Rationale:** One file per agent (not a directory-with-manifest like skills)
matches the layout the ecosystem already uses for agent definitions, so author
muscle memory and existing `.claude/agents/*.md` files drop in unchanged.
`name`/`description` are the minimum needed for a typed spawn choice; `model`
and `mode` let an agent encode its intended cost/capability profile so the LLM
need not restate it.

---

### REQ-AG-003: Frontmatter Separation

WHEN an agent definition is loaded
THE SYSTEM SHALL strip the YAML frontmatter block before using the file body as
the agent's persona

THE SYSTEM SHALL NOT include raw YAML frontmatter (`---` delimited blocks) in
the persona delivered to the sub-agent's system prompt

**Rationale:** Frontmatter is machine metadata for discovery and the spawn-tool
schema, not instructions for the model. Including it wastes context tokens and
confuses the persona with key-value pairs it cannot act on. This mirrors
REQ-SK-001 for skills.

---

### REQ-AG-004: Agent Type as a Typed Spawn Choice

THE SYSTEM SHALL expose discovered agents as an `agent_type` enumeration on the
`spawn_agents` tool's per-task schema, where each enumerated value carries the
agent's name and description

THE SYSTEM SHALL NOT inject an agent catalog into the system prompt text

**Rationale:** Models carry a strong, trained prior for selecting a named
sub-agent *type* as a parameter of the spawn/Task tool. Placing the agent
identity on the tool schema (rather than as prose in the system prompt, the way
skills are catalogued per REQ-SK-007) rides that prior, yielding more reliable
selection. Keeping the catalog out of the system prompt also avoids
representing the same list in two places, which would diverge and double the
cache-invalidation surface (see REQ-AG-008).

---

### REQ-AG-005: Spawn-Time Resolution and Precedence

WHEN a `spawn_agents` task names an `agent_type` that matches a discovered
agent
THE SYSTEM SHALL resolve the sub-agent's mode, model, and persona from the
agent definition

THE SYSTEM SHALL apply field precedence, highest first: an explicit value on
the task spec, then the agent definition's value, then the mode-based default

WHEN a task omits `agent_type`
THE SYSTEM SHALL resolve the sub-agent exactly as today (no persona; mode and
model from the task spec or mode-based defaults)

**Rationale:** Precedence makes the agent definition a *default-bearing* layer
between the LLM's explicit per-call overrides and the system's mode defaults.
The LLM can still override an agent's default model or mode for a one-off
without editing the file, and the existing anonymous-spawn path is untouched.

---

### REQ-AG-006: Persona Composition in the Sub-Agent System Prompt

WHEN a sub-agent is spawned from a named agent
THE SYSTEM SHALL use the agent's persona in place of the generic assistant
preamble at the head of the sub-agent's system prompt

THE SYSTEM SHALL retain the environment grounding (working directory, project
guidance, mode context) and the sub-agent result-submission suffix
(`submit_result` / `submit_error`) regardless of persona

WHEN a sub-agent's runtime is recreated during its run (for example, runtime
eviction on a model upgrade)
THE SYSTEM SHALL restore the persona so subsequent turns keep it rather than
falling back to the generic preamble

**Rationale:** A named agent's value is its persona *replacing* the default
"helpful assistant" framing — that is what differentiates a "security reviewer"
from a "docs writer." But the operational scaffolding a sub-agent needs to
function (where it is, what mode it is in, how it terminates) is orthogonal to
persona and must always be present, or the sub-agent cannot complete its
lifecycle. The persona is resolved at spawn but a sub-agent's in-memory context
is rebuilt when its runtime is recreated, so the persona is persisted with the
sub-agent conversation and restored on that path — otherwise a model-upgrade
eviction mid-run would silently demote a named agent to the generic prompt.

---

### REQ-AG-007: Unknown Agent Type Rejected

WHEN a `spawn_agents` task names an `agent_type` that does not match any
discovered agent
THE SYSTEM SHALL reject the spawn call with an error naming the unknown
`agent_type` and listing the available agent names

**Rationale:** A typed enum that silently accepts an unknown value would spawn a
persona-less sub-agent that the LLM believes has a persona — a silent
capability gap. Rejecting at spawn time, the same way an unknown `model` is
rejected (see `subagents.allium` `SpawnRejectedUnknownModel`), keeps the
mismatch visible and actionable.

---

### REQ-AG-008: Prompt-Cache Stability

THE SYSTEM SHALL render the `agent_type` enumeration deterministically (agents
ordered by name, stable serialization) so the spawn-tool definition is
byte-identical across turns of a single conversation

**Rationale:** Tool definitions sit in the cached prefix of the LLM request
(tools and system prompt are cached together under a per-conversation cache
key). The discovered agent set is fixed for a conversation's working directory,
so the tool definition is naturally stable across turns — *unless* the
enumeration is rendered from an unordered collection, in which case it varies
turn to turn and silently busts the cache on every request. Deterministic
ordering is what makes the cache hold.

---

### REQ-AG-009: Capability from Mode, Not Definition

THE SYSTEM SHALL derive a sub-agent's tool registry from its resolved mode
(Explore or Work), independent of which agent definition produced it

WHEN an agent definition declares a `tools` field
THE SYSTEM SHALL preserve it during parsing without acting on it

**Rationale:** Tool capability is governed solely by the Explore/Work mode
registries (`for_subagent_explore` / `for_subagent_work`); a named agent
changes persona and defaults, never which tools exist. Keeping capability
single-sourced in mode means there is exactly one registry-construction path to
reason about. The `tools` field is part of the on-disk format and is parsed and
preserved so the format can carry a capability declaration, but it is not part
of this contract: the resolved registry is a function of mode alone. (A
capability-restriction pass that consults `tools` is tracked as follow-up work,
not described here.)
