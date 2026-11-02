# User-Level Agent Plugins — Executive Summary

## Overview

Plugins are user-level-only capability bundles following the Agent Plugins
specification 1.1.0: a `plugin.json` manifest plus optional `skills/` and
`mcp.json` components, installed under the shared root
`~/.agents/plugins/<name>/`. Phoenix loads both component types through its
existing skill discovery and MCP client machinery; plugins add a bundling
and lifecycle layer (add/remove/enable/disable, containment, the stdio
runtime environment contract), not a new execution path.

[ADR-191](../adrs/191_agent-plugins-are-user-level-shared-root-namespaced-bundles.md)
records the scope decision (user-level only, shared root), the naming scheme
(`plugin:skill`, `<plugin>.<server>`), and the data-retention policy.

## Status

| Requirement | Title | Status | Notes |
|---|---|---|---|
| REQ-AP-001 | User-Level Install Scope | Complete | The only plugins root is the home-derived `~/.agents/plugins` (`default_plugins_root`); no working-directory or repository path is ever scanned |
| REQ-AP-002 | Installed-Plugin Discovery | Complete | `scan_plugins_root` one-level scan with manifest-name identity and duplicate-name first-seen-wins (`phoenix-plugins`); wired into MCP config discovery and the `GET /api/plugins` list (registry join in `plugin_list_entries`) |
| REQ-AP-003 | Manifest Validation | Complete | `Manifest::parse` (`phoenix-plugins`): exact 1.1.0 field set, unknown fields reported and ignored, non-object `extensions` ignored, unrecognized `$schema` rejects |
| REQ-AP-004 | Component Loading with Scoped Failure Boundaries | Partial | `load_plugin` with per-skill, per-server, and per-component-type scopes (`phoenix-plugins`), regression-tested from the specification's own examples; status surfacing in phase 4 |
| REQ-AP-005 | Plugin Skills | Complete | `SkillSource::Plugin` + `collect_plugin_skill` (`phoenix-skills`): `plugin:skill` namespacing derived at discovery, non-recursive trees, lowest precedence (scan after built-ins), conversation-audience only, frontmatter-name-matches-directory enforcement, `(plugin: name)` catalog provenance; invoked through the shared `invoke_skill` path |
| REQ-AP-006 | Plugin MCP Servers | Complete | stdio servers resolve the full environment contract and join the MCP merge keyed `<plugin>.<server>` after user config (`plugin_server_configs_under`, `merge_plugin_configs` in `phoenix-mcp`); streamable-http maps to the HTTP transport; `PLUGIN_DATA` create-before-admission; both surfaces observe the shared enabled-plugin filter |
| REQ-AP-007 | Containment | Complete | `within_boundary` (`phoenix-plugins`): realpath containment following symlinks, consistent lexical fallback for not-yet-existing paths, both plugin-root and data boundaries |
| REQ-AP-008 | Enable and Disable | Complete | process-wide plugin filter (`phoenix-plugins::filter`) is the single authority shared by skill discovery and the MCP merge; persisted in the `plugins` table (absent row = enabled), seeded at startup, written through `/api/plugins/:name/enable|disable` |
| REQ-AP-009 | Add and Remove Lifecycle | Complete | `POST /api/plugins/add` (local dir or shallow git clone, manifest validated before install, collision-refused, managed provenance registered); remove deletes the directory and drops the registry row retaining `PLUGIN_DATA`; purge-data is a separate action available for missing/removed roots; unmanaged directories load but are never modified without an explicit request |
| REQ-AP-010 | Status Surfacing | Complete | `GET /api/plugins` carries per-component outcomes (skills list, MCP server list, `mcp_disabled_reason`) plus missing-root entries; PluginsPage renders status glyph, provenance badge, counts, and MCP-disabled reasons inline |
| REQ-AP-011 | Extension Namespaces Reserved but Inert | Complete | All extension namespaces ignored without validation at manifest parse (`Manifest::parse` reports non-object `extensions` and ignores contents); `dev.scottopell.phoenix-ide/` reserved in ADR-191 |
| REQ-AP-012 | Version Recognition | Complete | Exact canonical 1.1.0 identifiers in `MANIFEST_SCHEMA_URL`/`MCP_SCHEMA_URL`; mixed-version packages disable MCP contribution (`McpDocument::parse`) |

## Cross-Spec References

- `specs/skills/` — discovery precedence, turn-scoped catalog advertisement
  (plugin skills ride REQ-SK-006 precedence and REQ-SK-009 refresh).
- `specs/mcp/` — transport-tagged config discovery and first-seen-wins merge
  (plugin servers extend REQ-MCP-001), live tool resolution (REQ-MCP-014),
  reload reconciliation (REQ-MCP-015), enable/disable persistence precedent
  (REQ-MCP-016).
