# ADR-052: Workers and tiers resolve usable model routes

- **Status:** Accepted
- **Date:** 2026-09-15
- **Affects:** REQ-AG-001 through REQ-AG-012; REQ-SA-007, REQ-SA-011

## Context

Production advertised named agents whose filesystem defaults referenced models
unavailable on the installation. A four-agent batch with omitted model overrides
failed on `opus`; an explicit registered Codex model succeeded. The caller had
followed the tool's defaulting guidance. This is a catalog admission failure,
not merely an invented model name.

The user reaffirmed the August 22 configuration design: one XDG TOML file,
inline worker instructions, ordered model/effort candidates, and no agent-owned
mode or tools. Design PR #705 was closed over scope and ownership, not rejection
of those decisions. The current discussion adds configurable tiers and explicit
model selection with provider-connection identity. Issue #651 coordinates the
workstream; feature specs remain the behavioral authority.

## Options considered

1. **Validate filesystem-agent defaults.** Narrowly prevents the observed hidden
   default failure, but keeps contextual file discovery and multiple defaulting
   layers the user intends to retire.
2. **Named workers only.** Hides models effectively, but prevents the LLM from
   composing an anonymous worker or honoring an exact model request.
3. **One config, optional workers, and one execution selector.** Keeps reusable
   personas while supporting generic tier and exact-route selection. Requires
   explicit connection and effort capability handling.

## Decision

Adopt option 3. Load version-1 `$XDG_CONFIG_HOME/phoenix-ide/config.toml` (with
`$HOME/.config` fallback) once per parent runtime. Named workers carry description,
inline instructions, and optional ordered execution candidates. Tiers carry
ordered candidates without a persona. Retire filesystem-agent discovery and its
format/frontmatter requirements REQ-AG-001/002/003; REQ-AG-010 is the sole source
contract. Preserve the retired requirement text for historical traceability.

A task may name a worker independently of one optional execution selector:
configured tier, or exact model/connection with optional effort. Omission selects
the worker's candidate list when present, otherwise inherits the parent's whole
model/connection/effort choice. Explicit model selection with omitted effort uses
that model's native default, never the parent's effort. Authority is independent.

Candidates bind model, logical provider connection, and effort atomically.
Unavailable routes are skipped in order. A reached available candidate with known
incompatible effort invalidates the affected selection and produces a diagnostic.
Only eligible workers, tiers, model routes, and supported explicit effort choices
are advertised. An unknown capability is not evidence for advertising an effort
as supported. A parent using Codex may spawn through another configured usable
connection; provider family does not impose a child restriction.

Refresh usable routes at each parent request boundary. Build the schema and
admit its response using the same resolved catalog snapshot. Persist the selected
model, effort, persona, and connection before running the child; runtime recreation
restores them without consulting config again. If that route disappears, fail
rather than silently substituting. No provider health probes, outage retry system,
config watcher, or parallel filesystem fallback catalog are introduced.

Connection identity names the configured backend slot (`codex`, `anthropic`,
`openai_responses`, `openai_chat_completions`, or `mock`). It does not promise
endpoint/account identity after an operator reconfigures that slot. Existing
registry aliases distinguish models registered through different backends.

## Consequences

- **Positive:** Advertised worker defaults cannot contain a known unavailable
  route; generic delegation can choose cost/capability without guessing model IDs.
- **Positive:** Exact requests stay exact, and execution preferences cannot grant
  write access or silently carry effort between models.
- **Negative:** Users must deliberately transfer desired filesystem-agent
  instructions into TOML. No migration engine or automatic import is provided;
  old files are left untouched and are not an active catalog.
- **Negative:** A bad configuration can remove a worker or tier from callable
  choices, so the diagnostic must identify the selection and reason.
- **Neutral:** Config edits apply when the parent runtime is created again.
  Route availability refresh can alter tool definitions and their cache prefix.
- **Neutral:** The selected connection needs one narrow persisted child binding.
  Prefeature children without that binding retain their existing behavior. This
  does not expand restart survival, rollback, or live-replacement guarantees.
- **Neutral:** Skills remain reusable expertise. Plugin hosting is outside scope.

## References

- [Named-agent requirements](../agents/requirements.md)
- [Sub-agent requirements](../subagents/requirements.md)
- [Configuration example](../../docs/agents-config.md)
- [ADR-034: explicit compatibility guarantees](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- [Design PR #705](https://github.com/scottopell/phoenix-ide/pull/705)
- `SpawnAgentsTool::input_schema`, `handle_spawn_agents_tool`, `create_subagent_conversation`
