# Named Agents — Executive Summary

## Requirements Summary

Named workers are optional reusable personas defined inline in one versioned XDG
TOML file. Workers and tiers carry ordered model/connection/effort preferences.
The LLM can choose a named worker, a tier, or an exact model route; choosing a
worker and execution are independent. Generic omission inherits the parent's
whole execution choice. Worker definitions do not carry mode or tools.

The callable catalog excludes unusable defaults and diagnoses configuration
mistakes. Connection availability reflects configured backend routes, not model
family names. Exact selections do not silently fall back. Filesystem-agent
catalogs are retired; Skills remain the home for reusable expertise.

## Technical Summary

[ADR-052](../adrs/052_workers-and-tiers-resolve-usable-model-routes.md) records the
configuration and model-route boundary. Configuration is loaded once per parent
runtime; usable routes refresh at request boundaries. The same resolved snapshot
renders the tool schema and admits its response. Resolved child persona and
execution are persisted for runtime recreation, without adding server-restart
survival guarantees or connection endpoint/account lineage.

[Configuration examples](../../docs/agents-config.md) show the public TOML and
spawn selectors. [agents.allium](agents.allium) models catalog preparation and
persona composition; [subagents.allium](../subagents/subagents.allium) owns spawn
validation, authority, and child lifecycle seams.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| **REQ-AG-001:** Agent Definition Discovery | Retired | Filesystem discovery replaced by REQ-AG-010; ADR-052 |
| **REQ-AG-002:** Agent Definition Format | Retired | Markdown frontmatter replaced by inline TOML; ADR-052 |
| **REQ-AG-003:** Frontmatter Separation | Retired | Inline instructions require no frontmatter parsing; ADR-052 |
| **REQ-AG-004:** Agent Type as a Typed Spawn Choice | In progress | Eligible worker enum and diagnostics require implementation validation |
| **REQ-AG-005:** Spawn-Time Resolution and Precedence | In progress | Worker/tier/explicit selection and parent inheritance contract updated |
| **REQ-AG-006:** Persona Composition | Implemented baseline | Persisted persona restoration exists; inline-config integration requires validation |
| **REQ-AG-007:** Unknown Agent Type Rejected | Implemented baseline | Atomic rejection exists; resolved catalog integration requires validation |
| **REQ-AG-008:** Catalog Snapshot Consistency | In progress | Per-runtime config and per-request availability snapshot |
| **REQ-AG-009:** Capability from Spawn Authority, Not Definition | In progress | Remove worker-owned mode/tools parsing |
| **REQ-AG-010:** Single User Configuration | In progress | Version-1 XDG TOML and filesystem retirement |
| **REQ-AG-011:** Ordered Atomic Execution Candidates | In progress | Availability fallback; incompatible effort diagnosed |
| **REQ-AG-012:** Usable Model Routes | In progress | Logical backend connection identity through child creation/recreation |

**Progress:** 3 retired requirements; 2 existing behavioral baselines; 7 requirements
under implementation. No completion claim is made before targeted validation.

## Verification targets

- Codex-only routes cannot advertise an Anthropic-only worker default.
- Ordered candidates skip unavailable routes but diagnose known invalid effort.
- Worker instructions survive explicit execution overrides and runtime recreation.
- Generic inheritance carries model, connection, and effort together.
- Invalid selection rejects a whole batch without partial child creation.
- Config edits do not alter an active runtime's loaded definitions or an active
  child's resolved execution; request catalog availability remains consistent.
