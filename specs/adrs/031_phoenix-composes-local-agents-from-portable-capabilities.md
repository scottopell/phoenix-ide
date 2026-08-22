# ADR-031: Phoenix composes local named agents from portable capabilities

- **Status:** Proposed
- **Date:** 2026-08-22
- **Affects:** REQ-AG-001 through REQ-AG-009; REQ-SK-001 through REQ-SK-007; named-agent configuration; Agent Plugins adoption

## Context

Phoenix discovers named agents from Markdown files under `.claude/agents` and
`.agents/agents`. Those files combine several concerns: reusable instructions,
model defaults, execution-authority defaults, an inert tool list, filesystem
precedence, and typed `agent_type` selection. The resulting object resembles a
portable package component but also embeds Phoenix-specific execution policy.

Phoenix needs a user-level way to name a reusable worker composition and choose
model-and-reasoning-effort preferences that vary across installations. One
installation may provide only Codex-backed models while another provides GLM or
other externally registered models. The composition must therefore permit an
explicit ordered set of atomic model-and-effort candidates without inferring
substitutes from model names.

The Agent Plugins 1.0.0 portable core exposes Skills and MCP servers. This
decision relies only on that portable component boundary and treats Phoenix
named agents as outside it. Plugin installation, client extensions, and any
future expansion of the external standard are outside this decision.

The decision intersects active repository-authority, ProductConversation, and
durable-workflow work. Changing the normative agent and skill contracts before
those interfaces settle would create avoidable cross-workstream churn. A
point-in-time decision can establish direction without prematurely changing the
current behavioral specifications.

## Options considered

1. **Keep filesystem named agents and add the same fields to user
   configuration.** This preserves project-local agent sharing and the existing
   discovery path, but creates two authoritative representations, requires
   precedence and field-parity rules, and retains mode/tool fields that mix
   composition with authority.
2. **Treat named agents as portable Agent Plugin components.** This would keep
   compositions project-distributable and colocated with Skills, but would
   invent a non-standard core component whose lifecycle, model, authority, and
   tool semantics are specific to Phoenix. Other conforming clients could not
   consume it portably, and Phoenix would own a plugin extension before proving
   ecosystem demand for it.
3. **Remove named agents and use Skills alone.** This maximizes portability and
   reduces concepts, but Skills do not answer which model and reasoning effort
   Phoenix should use for a delegated worker, nor do they provide a stable typed
   worker choice to the coordinator.
4. **Keep named agents as Phoenix-local user compositions and adopt Agent
   Plugins independently for portable Skills and MCP.** User configuration owns
   the local composition; plugins own portable knowledge and capabilities; the
   spawn request independently chooses Work or Explore authority. This removes
   project-local named-agent portability and requires migration from the
   existing filesystem format, but avoids a parallel representation and keeps
   Phoenix execution policy out of the portable plugin core.

## Decision

Phoenix chooses option 4 as the direction for the later named-agent cutover.
This Proposed ADR records that direction; it does not change current behavior or
supersede the normative agent and skill specifications.

When the cutover is specified and implemented, filesystem-based named-agent
discovery is intended to be retired. Named agents are intended to become
Phoenix-local user compositions loaded from one versioned TOML file under the
effective user's XDG configuration directory. The first configuration schema is
intended to contain only inline named-agent definitions and no external
instruction-file references.

The intended configured named-agent shape contains:

- a name supplied by its TOML table key;
- a non-empty description shown to the coordinator;
- non-empty inline instructions used as the child persona; and
- an optional ordered, non-empty list of execution candidates, where each
  candidate atomically pairs a registered model identifier with an optional
  reasoning-effort value.

Under the intended resolution policy, candidate order is explicit user policy.
Phoenix selects the first candidate whose model is available. An absent model
permits trying the next candidate. A present model with an incompatible explicit
effort is a configuration error, not a reason to silently fall through. If the
list is omitted, existing Work/Explore defaults apply. The resolved
instructions, model, and effort are snapshotted for an admitted child so later
configuration edits do not change that child during recreation or resume.

The intended named-agent contract does not contain Work/Explore mode, execution
authority, tool lists, Skill allowlists, plugin references, provider
credentials, provider endpoints,
model-registry declarations, concurrency policy, or parallel-work capability.
Work/Explore remains a delegation choice. Tool availability and actual write
authority remain runtime admission decisions. The `agent_type` tool-schema
affordance remains, but its catalog is derived from validated user
configuration rather than filesystem agent discovery.

Agent Plugins adoption is intended to proceed as an independent portability
layer. Portable plugins provide Skills and MCP servers; they do not provide
Phoenix named agents. The intended initial adoption is Skills-first, using
Phoenix's existing Skill catalog and invocation semantics. MCP plugin hosting,
plugin installation, enablement, trust, update policy, and Phoenix-specific
extensions remain separate product decisions.

Project-specific reusable expertise should be represented as a Skill.
Repository-wide standing constraints remain repository guidance such as
`AGENTS.md`. Phoenix may compose available Skill knowledge when running a named
agent, but the first named-agent schema will not statically reference Skills.

Current normative requirements continue to require filesystem named-agent
discovery. This ADR remains non-binding while Proposed. Requirements, Allium,
implementation, and migration policy must change together before the cutover can
alter product behavior.

## Consequences

- **Positive:** Phoenix has one authoritative source for named-agent
  composition instead of parallel filesystem and TOML definitions.
- **Positive:** agent definitions cannot grant themselves write access or tool
  capabilities; delegation and runtime admission retain those authorities.
- **Positive:** ordered model-and-effort candidates let one conceptual agent
  work across installations with different registered providers without
  model-name heuristics.
- **Positive:** Agent Plugins remain portable because Phoenix-specific model,
  authority, and lifecycle policy does not leak into the plugin core.
- **Positive:** project knowledge can travel as Skills independently of the
  Phoenix-local model used to execute it.
- **Negative:** repositories lose version-controlled, project-local named-agent
  files and their walk-up override behavior.
- **Negative:** existing filesystem agents require visible migration guidance;
  silently ignoring mode, tools, or persona content could materially change
  delegated behavior.
- **Negative:** a machine-local TOML composition is not portable to clients
  that do not implement Phoenix configuration.
- **Negative:** explicit fallback candidates add availability and diagnostic
  states that the agent catalog and tool schema must represent coherently.
- **Neutral:** Phoenix continues to persist or snapshot the resolved child
  persona and execution choice; only the source of future named-agent
  definitions changes.
- **Neutral:** Agent Plugins installation and MCP support are not prerequisites
  for user-configured named agents, and named-agent configuration is not a
  prerequisite for Skills-first plugin loading.

## References

- [Agent Plugins 1.0.0 specification](https://agent-plugins.org/llms.txt)
- `AgentDefinition`, `discover_agents_with_home`, and
  `parse_agent_frontmatter` in `phoenix-agents`
- `discover_skills`, `invoke_skill`, and `SkillMetadata` in `phoenix-skills`
- `ToolRegistryExecutor::handle_spawn_agents_tool`
- `build_system_prompt`
- `specs/agents/requirements.md`
- `specs/agents/agents.allium`
- `specs/skills/requirements.md`
- `specs/skills/skills.allium`
- ADR-013, durable workflows use normalized core and typed profiles
- ADR-026, ProductConversation lifecycle is separate from WorkScope resource ownership
