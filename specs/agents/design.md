# Named Agents — Technical Design

## Architecture Overview

A named agent is a single Markdown file with YAML frontmatter (metadata) and a
body (persona instructions). Agents are discovered from the working-directory
tree, surfaced to the LLM as a typed `agent_type` choice on the `spawn_agents`
tool, and — when a spawn task selects one — supply the spawned sub-agent's
persona, default model, and default mode.

Named agents are a *thin layer over the existing sub-agent machinery*. They add
no new conversation states, no new spawn lifecycle, and no new isolation rules.
Everything in [`subagents.allium`](../subagents/subagents.allium) and
[`bedrock.allium`](../bedrock/bedrock.allium) continues to govern how a
sub-agent runs; agents only change *what persona it runs with* and *which
defaults it starts from*.

```
discovery (phoenix-agents)          spawn tool (phoenix-tools)
  .claude/agents/*.md  ─┐            SpawnAgentsTool::with_agents(catalog)
  .agents/agents/*.md  ─┼─▶ catalog ─▶  input_schema(): agent_type enum
  walk-up + children   ─┘   (sorted)         │
  + $HOME, deduped                           ▼
                                   LLM picks { task, agent_type, ... }
                                             │
                              executor: resolve persona + defaults
                              (task field > agent def > mode default)
                                             │
                                             ▼
                              SubAgentSpec { ..., agent_name, persona }
                                             │
                              system_prompt: persona replaces base preamble,
                              grounding + submit suffix retained
```

## The `phoenix-agents` crate

A new leaf crate mirroring `phoenix-skills`: it depends only on `phoenix-core`
domain types, and the discovery/parsing logic is structurally the same walk as
skills (the two share the same tree-walk, symlink/content/name dedup, and
`$HOME` fallback). Keeping it a separate leaf crate — rather than folding agents
into `phoenix-skills` — keeps each crate's domain singular (a skill is an
invocable instruction set; an agent is a sub-agent persona) and avoids a
crate named for one concept owning two.

```rust
const AGENT_DIRS: &[&str] = &[".claude/agents", ".agents/agents"];

pub struct AgentDefinition {
    pub name: String,            // frontmatter `name`; the agent_type value
    pub description: String,     // frontmatter `description`; shown on the enum
    pub body: String,            // file body, frontmatter stripped — the persona
    pub path: PathBuf,           // absolute path to the .md file
    pub source: String,          // ".claude/agents" | ".agents/agents"
    pub model: Option<String>,   // frontmatter `model` — default model id
    pub mode: Option<SubAgentMode>, // frontmatter `mode` — explore | work
    // `tools` is parsed and preserved but inert in v1 (REQ-AG-009).
}

/// Discover agents for a working directory. Result is sorted by name and
/// fully deduplicated (symlink, content-hash, then first-name-wins).
pub fn discover_agents(working_dir: &Path) -> Vec<AgentDefinition>;
```

## Agent file format (REQ-AG-002, REQ-AG-003)

```
---
name: security-reviewer
description: Reviews code for vulnerabilities, credential handling, and attack vectors
model: claude-sonnet-4-6
mode: explore
---

You are a security-focused code reviewer. For the code in scope, look for
injection vectors, unsafe credential handling, missing authz checks, and
unvalidated input. Report findings with severity and a concrete fix.
```

| Field | Required | Type | Purpose |
|-------|----------|------|---------|
| `name` | yes | string | Agent identifier; the `agent_type` enum value |
| `description` | yes | string | One-line description shown on the enum choice |
| `model` | no | string | Default model id (must exist in the LLM registry) |
| `mode` | no | `explore`\|`work` | Default sub-agent mode |
| `tools` | no | list | Parsed and preserved, inert in v1 (REQ-AG-009) |

Frontmatter stripping reuses the skill `strip_frontmatter` approach: everything
after the closing `---` delimiter is the persona; a file without a leading
`---\n` is treated as all-body.

## Discovery (REQ-AG-001)

Identical shape to skill discovery (`phoenix-skills`):

1. `.claude/agents/` and `.agents/agents/` at each level from CWD to root
2. Immediate children of CWD (projects-directory case)
3. `$HOME/.claude/agents/` and `$HOME/.agents/agents/` when `$HOME` is not an
   ancestor of CWD

Dedup: canonical path (symlinks), content hash (copies), name (first-seen,
closest-to-CWD wins). The returned `Vec` is sorted by name — this sort is what
satisfies the prompt-cache stability requirement (REQ-AG-008), so it is part of
the contract, not an incidental nicety.

Agents are *not* discovered recursively into a `agents/` sub-directory the way
skills namespace sub-skills — there is no `agent:subagent` namespacing. One flat
level per agents directory.

## Schema-enum injection (REQ-AG-004, REQ-AG-008)

`SpawnAgentsTool` becomes stateful so its `input_schema()` can render the
discovered agents. It is constructed per-conversation, capturing the catalog,
at the point where the tool registry is built:

```rust
pub struct SpawnAgentsTool {
    agents: Vec<AgentDefinition>, // sorted by name; empty when none discovered
}

impl SpawnAgentsTool {
    pub fn with_agents(agents: Vec<AgentDefinition>) -> Self { Self { agents } }
}
```

`input_schema()` adds `agent_type` to the per-task object:

```jsonc
"agent_type": {
  "type": "string",
  "enum": ["docs-writer", "security-reviewer"],   // sorted names
  "description": "Named agent persona to spawn. One of:\n- security-reviewer: Reviews code for vulnerabilities...\n- docs-writer: ..."
}
```

When no agents are discovered, `agent_type` is omitted from the schema entirely
— the tool keeps its current shape, so the no-agents case is a strict subset of
today's behaviour.

The `Tool` trait is unchanged: `input_schema(&self) -> Value` still has no
working-directory parameter. The catalog is captured at construction time
instead, which is sufficient because the registry is already built fresh
per-conversation and a conversation's working directory is fixed.

### Prompt-cache interaction

The LLM request caches `tools + system` together under a per-conversation cache
key. Because the agent catalog is fixed for a conversation's working directory,
the rendered `agent_type` enum is constant across that conversation's turns —
*provided* it is serialized deterministically. The discovery `Vec` is sorted by
name, and the enum/description strings are built by iterating that sorted `Vec`,
so the schema bytes are stable turn to turn. An unordered source (e.g. iterating
a `HashSet`) would re-order the enum per turn and bust the cache on every
request; REQ-AG-008 forbids that. The agent catalog is deliberately *not* also
emitted into the system-prompt text (REQ-AG-004), so there is a single
cache-bearing representation, not two.

## Spawn-time resolution and precedence (REQ-AG-005)

`SubAgentTask` (the LLM-supplied per-task shape, owned by `subagents.allium`)
gains an optional `agent_type`. The executor's per-task resolution
(`subagents.allium` `SubAgentSpecsResolved`) consults the resolved agent as the
middle precedence layer:

```
resolve_agent(t)    = find_agent(catalog, t.agent_type)   // None if absent
resolve_mode(t)     = t.mode ?? agent.mode ?? explore
resolve_model_id(t) = t.model ?? agent.model ?? mode_default_model(resolved_mode, parent)
resolve_persona(t)  = agent.body                          // None when no agent
```

`SubAgentSpec` gains `agent_name: Option<String>` and `persona: Option<String>`
so the resolved persona is threaded to the runtime that builds the sub-agent's
system prompt. Omitting `agent_type` produces `agent = None`, and resolution
collapses to exactly today's behaviour.

Mode validation (`WorkSubAgentRequiresWriteableParent`) runs against the
*resolved* mode, so an agent whose default `mode: work` is spawned from an
Explore parent is rejected the same as an explicit `mode: "work"` would be.

## Persona composition (REQ-AG-006)

`build_system_prompt` already assembles: base preamble → project guidance →
mode context → sub-agent suffix. When the sub-agent carries a persona, the
persona *replaces the base preamble* (the generic "you are a helpful assistant"
framing); guidance, mode context, and the `submit_result`/`submit_error` suffix
are unchanged. This is what makes a named agent feel like a distinct
persona while keeping every sub-agent operationally complete.

The persona is threaded into `build_system_prompt` as an optional argument
sourced from `SubAgentSpec.persona`; absent persona ⇒ the current base preamble.

### Persona persistence across runtime recreation

The persona is resolved at spawn and set on the fresh `ConvContext`. A
sub-agent's runtime can be recreated mid-run (e.g. model-upgrade eviction),
which rebuilds `ConvContext` from the database — where the persona would not
otherwise exist, so remaining turns would fall back to the generic prompt. To
prevent that silent demotion, the persona is persisted in a dedicated
`sub_agent_personas` table (keyed by conversation id, `ON DELETE CASCADE`) at
spawn and re-read into `ConvContext` on the resume path. A dedicated table —
rather than columns on `conversations` — keeps this sub-agent-only metadata off
the row that the overwhelming majority of conversations would leave NULL. The
spawn-time write is best-effort: the live context already carries the persona,
so a write failure only degrades a later resume and is logged.

## Capability from mode, not definition (REQ-AG-009)

The sub-agent's tool registry is selected purely by resolved mode
(`for_subagent_explore` / `for_subagent_work`), exactly as in `subagents.allium`
`SubAgentRegistryOnFreshSpawn`. A named agent changes persona and defaults, not
which tools exist. The `tools` frontmatter field is parsed into
`AgentDefinition` and otherwise untouched, so a future capability-restriction
pass can act on it without a format migration.

## Relationship to the sub-agents spec (the seam)

| Concern | Owner |
|---------|-------|
| Discovery, frontmatter, persona | `agents.allium` (this spec) |
| `agent_type` schema enum + stability | `agents.allium` (this spec) |
| Persona composition in system prompt | `agents.allium` (this spec) |
| Spawn validation incl. unknown `agent_type` | `subagents.allium` |
| Mode/model/turn defaulting, cwd-scoping, one-writer | `subagents.allium` |
| Spawn lifecycle, fan-in, cancellation, timeout | `bedrock.allium` |

`subagents.allium` gains `agent_type` on `SubAgentTask`, `agent_name`/`persona`
on `SubAgentSpec`, and the `SpawnRejectedUnknownAgentType` rule; its resolution
guidance references the precedence above. No other sub-agent behaviour changes.

## User-initiated agent invocation (out of scope)

A user typing an agent name to launch it directly (the analogue of `/skill`) is
deliberately out of scope. Named agents are a spawn-time persona for the LLM's
delegation, not a user-facing slash command. This keeps v1 to a single
invocation path and avoids the delivery-format split skills carry (REQ-SK-002).

## Testing strategy

### Unit tests
- Frontmatter parsing: valid, missing `name`, missing `description`, no
  frontmatter, with/without `model`/`mode`/`tools`.
- Discovery: project agents, user agents, child-directory agents, symlink and
  content dedup, name override (closest-to-CWD wins).
- Sort stability: discovery output is byte-stable across repeated runs over the
  same tree (the REQ-AG-008 guarantee).
- Schema rendering: enum present and sorted when agents exist; `agent_type`
  absent from schema when none discovered.
- Resolution precedence: task field beats agent def beats mode default for mode
  and model.

### Integration tests
- Spawn with `agent_type`: persona replaces base preamble; grounding + submit
  suffix retained.
- Unknown `agent_type`: spawn rejected with available-names error.
- Agent `mode: work` from an Explore parent: rejected by
  `WorkSubAgentRequiresWriteableParent`.
- No-agents working dir: spawn schema and behaviour identical to today.

### Property tests
- Arbitrary agent trees: discovery output is always sorted and name-unique.
- Frontmatter stripping never leaves a leading `---\n` in the persona.
