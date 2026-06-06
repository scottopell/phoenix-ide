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
  result-submission suffix are retained.
- **Capability** stays single-sourced in the Explore/Work mode registries; the
  `tools` field is parsed-and-preserved for forward compatibility.

## Status Summary

Specification only; no implementation exists yet.

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AG-001:** Agent Definition Discovery | 📋 Planned | Mirrors skill discovery in a new `phoenix-agents` crate |
| **REQ-AG-002:** Agent Definition Format | 📋 Planned | One `.md` per agent; `name`/`description` required |
| **REQ-AG-003:** Frontmatter Separation | 📋 Planned | Reuses skill `strip_frontmatter` approach |
| **REQ-AG-004:** Agent Type as Typed Spawn Choice | 📋 Planned | `agent_type` enum on `spawn_agents`; no system-prompt catalog |
| **REQ-AG-005:** Spawn-Time Resolution and Precedence | 📋 Planned | Threads `agent_type` through `subagents.allium` resolution |
| **REQ-AG-006:** Persona Composition | 📋 Planned | Persona replaces base preamble; grounding + suffix retained |
| **REQ-AG-007:** Unknown Agent Type Rejected | 📋 Planned | `SpawnRejectedUnknownAgentType` in `subagents.allium` |
| **REQ-AG-008:** Prompt-Cache Stability | 📋 Planned | Deterministic, name-sorted enum rendering |
| **REQ-AG-009:** Capability from Mode, Not Definition | 📋 Planned | `tools` field preserved but inert in v1 |

**Progress:** 0 of 9 implemented (specification under review).

## Deferred refinements

- **Per-agent tool allowlist:** an agent definition could narrow the sub-agent's
  toolset below its mode default. The `tools` frontmatter field is
  parsed-and-preserved so this is a non-breaking future addition (REQ-AG-009).
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
