# Named Agents — Executive Summary

## Requirements Summary

Named workers are optional reusable personas defined inline in one versioned XDG
TOML file. Workers and tiers carry ordered model/connection/effort preferences.
The LLM can choose a named worker, a tier, or an exact model route; choosing a
worker and execution are independent. Generic omission inherits the parent's
whole execution choice. Worker definitions do not carry mode or tools.

The callable catalog excludes unusable defaults and diagnoses configuration
mistakes. Connection availability reflects configured backend routes, not model
family names. Exact selections do not silently fall back. Retired built-in pins
use explicit compatibility replacements on the same connection, fail when that
replacement route is unavailable, and defer to an exact operator-configured
route. Filesystem-agent catalogs are retired; Skills remain the home for reusable
expertise.

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
| **REQ-AG-001:** Agent Definition Discovery | Retired | Filesystem discovery replaced by REQ-AG-010; original contract archived in ADR-052 |
| **REQ-AG-002:** Agent Definition Format | Retired | Markdown frontmatter replaced by inline TOML; original contract archived in ADR-052 |
| **REQ-AG-003:** Frontmatter Separation | Retired | Inline instructions require no frontmatter parsing; original contract archived in ADR-052 |
| **REQ-AG-004:** Agent Type as a Typed Spawn Choice | Implemented | `codex_only_catalog_prevents_hidden_opus_failure` verifies filtering and advertised choices |
| **REQ-AG-005:** Spawn-Time Resolution and Precedence | Implemented | `generic_omission_inherits_parent_execution`; `override_replaces_execution_and_keeps_persona` |
| **REQ-AG-006:** Persona Composition | Implemented | `agent_type_resolves_from_loaded_config`; `unattached_sub_agent_persists_selection_without_parent_effort_leak` verifies persisted persona |
| **REQ-AG-007:** Unknown Agent Type Rejected | Implemented | `rejects_unknown_agent_type`; `unknown_model_on_later_task_rejected_before_any_spawn` |
| **REQ-AG-008:** Catalog Snapshot Consistency | Implemented | `advertisement_snapshot_does_not_reresolve_a_worker`; `schema_is_byte_stable_across_calls` |
| **REQ-AG-009:** Capability from Spawn Authority, Not Definition | Implemented | `rejects_invalid_and_legacy_fields`; `mode_guidance_separates_permissions_from_execution` |
| **REQ-AG-010:** Single User Configuration | Implemented | `config_location_uses_only_xdg_or_home`; `missing_config_does_not_load_legacy_files` |
| **REQ-AG-011:** Ordered Atomic Execution Candidates | Implemented | `parses_ordered_atomic_candidates_and_inline_instructions`; `invalid_reached_effort_does_not_fall_through` |
| **REQ-AG-012:** Usable Model Routes | Implemented | `execution_routes_follow_connections_not_display_families`; `retired_pin_requires_replacement_on_the_same_connection_without_fallback`; `configured_exact_route_precedes_legacy_pin_replacement` |

**Progress:** 9 active requirements implemented; 3 filesystem requirements retired.

## Verification

Validation on devmbp passed the full Rust suite, compilation, code generation,
generated-file staleness, Allium, spec shape, spec anchors, end-to-end tests, and
dev.py tests at `caab8290d`.

The tests above cover unavailable defaults, atomic candidate resolution, exact
connection selection, parent inheritance, persona persistence, and shared catalog
snapshots. `schema_pins_each_model_to_its_connection_and_efforts` checks the
advertised route/effort combinations; `sub_agent_selection_failure_rolls_back_conversation_and_persona`
checks that persistence failure leaves no partial child or persona.
