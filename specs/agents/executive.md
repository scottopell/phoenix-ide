# Named Agents — Executive Summary

## Requirements Summary

Named agents are reusable sub-agent personas stored as single Markdown files
(`.claude/agents/*.md` or `.agents/agents/*.md`) with YAML frontmatter
(`name`, `description`, optional `model` and `mode`) and a body that is the
agent's persona instructions. They are discovered from the working-directory
tree using the same walk-up, child-directory, `$HOME`, and dedup rules as
skills. Discovered agents are surfaced to the LLM as a typed `agent_type`
enumeration on the `spawn_agents` tool — not as system-prompt prose — so the
model's trained prior for selecting a named sub-agent type drives reliable
selection. When a spawn task selects an `agent_type`, the agent definition
supplies the spawned sub-agent's persona (replacing the generic preamble) and
its default model and mode, with explicit task fields taking precedence over the
definition and the definition taking precedence over mode defaults. An unknown
`agent_type` is rejected at spawn time. The agent enumeration is rendered
deterministically so the cached spawn-tool definition is stable across a
conversation's turns. Tool capability remains governed by Explore/Work mode;
the `tools` frontmatter field is preserved but inert.

## Technical Summary

The detailed behaviour is normative in [`agents.allium`](./agents.allium)
(discovery, frontmatter, schema-enum, resolution, persona) and
[`subagents.allium`](../subagents/subagents.allium) (spawn validation, including
the unknown-`agent_type` rejection, and the resolution precedence that threads
the persona into `SubAgentSpec`).

- **Discovery** lives in a new `phoenix-agents` leaf crate that mirrors
  `phoenix-skills` — same tree walk, same symlink/content/name dedup, same
  `$HOME` fallback — returning a `Vec<AgentDefinition>` sorted by name.
- **Schema-enum** injection makes `SpawnAgentsTool` stateful
  (`SpawnAgentsTool::with_agents(catalog)`), constructed per-conversation where
  the registry is built; `input_schema()` renders a sorted `agent_type` enum
  with per-value descriptions, or omits it entirely when no agents exist.
- **Resolution** extends `subagents.allium`'s `SubAgentSpecsResolved`:
  `SubAgentTask` gains `agent_type`; `SubAgentSpec` gains `agent_name` and
  `persona`; precedence is task field → agent definition → mode default.
- **Persona composition** threads the persona into `build_system_prompt`, where
  it replaces the base preamble while grounding, mode context, and the
  result-submission suffix are retained. The persona is persisted in a
  dedicated `sub_agent_personas` table and restored when a sub-agent runtime is
  recreated mid-run, so a model-upgrade eviction does not demote a named agent
  to the generic prompt.
- **Capability** stays single-sourced in the Explore/Work mode registries; the
  `tools` field is parsed-and-preserved for forward compatibility.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AG-001:** Agent Definition Discovery | ✅ Implemented | `phoenix_agents::discover_agents` (walk-up + children + `$HOME`, sorted, deduped) |
| **REQ-AG-002:** Agent Definition Format | ✅ Implemented | One `.md` per agent; `parse_agent_frontmatter` requires `name`/`description` |
| **REQ-AG-003:** Frontmatter Separation | ✅ Implemented | `phoenix_agents::strip_frontmatter` |
| **REQ-AG-004:** Agent Type as Typed Spawn Choice | ✅ Implemented | `SpawnAgentsTool::with_agents` renders the `agent_type` enum in `input_schema` |
| **REQ-AG-005:** Spawn-Time Resolution and Precedence | ✅ Implemented | `handle_spawn_agents_tool` resolves task → agent def → mode default |
| **REQ-AG-006:** Persona Composition | ✅ Implemented | `build_system_prompt` persona arg; persisted in `sub_agent_personas` and restored on resume |
| **REQ-AG-007:** Unknown Agent Type Rejected | ✅ Implemented | `handle_spawn_agents_tool` rejects an unmatched `agent_type` before spawning |
| **REQ-AG-008:** Prompt-Cache Stability | ✅ Implemented | Discovery sorts by name and dir entries sort before dedup; schema byte-stable |
| **REQ-AG-009:** Capability from Mode, Not Definition | ✅ Implemented | Registry selected by mode; `tools` frontmatter parsed and preserved, not consulted |

**Progress:** 9 of 9 implemented.

## Deferred refinements

- **Per-agent tool allowlist:** an agent definition could narrow the sub-agent's
  toolset below its mode default. The `tools` frontmatter field is parsed and
  preserved so this can be added without a format change. Tracked as follow-up
  work, not part of the current contract.
- **User-initiated agent invocation:** launching a named agent directly from the
  user (the analogue of `/skill`) is out of scope; named agents are a spawn-time
  persona for LLM delegation only.

## Cross-Spec References

- `specs/subagents/` — owns the spawn lifecycle, mode/model/turn defaulting,
  one-writer and cwd-scoping invariants, and (extended by this feature) the
  unknown-`agent_type` rejection and persona threading.
- `specs/bedrock/` — owns the conversation state machine that runs every
  sub-agent regardless of persona.
- `specs/skills/` — the discovery and frontmatter pattern named agents mirror;
  REQ-SK-006 (discovery) and REQ-SK-001 (frontmatter) are the direct analogues
  of REQ-AG-001 and REQ-AG-003.
