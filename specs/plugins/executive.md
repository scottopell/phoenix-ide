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
| REQ-AP-001 | User-Level Install Scope | Not started | No project-level discovery path may exist; enforced by `PluginsAreUserLevelOnly` |
| REQ-AP-002 | Installed-Plugin Discovery | Not started | One-level scan of the shared root, manifest-name identity, duplicate-name first-seen-wins |
| REQ-AP-003 | Manifest Validation | Not started | Exact 1.1.0 field set; unknown fields reported and ignored; unrecognized `$schema` rejects |
| REQ-AP-004 | Component Loading with Scoped Failure Boundaries | Not started | Per-component and per-server failure isolation; wrong-kind component locations invalidate only that component type |
| REQ-AP-005 | Plugin Skills | Not started | `plugin:skill` namespacing, non-recursive skills tree, lowest precedence under user skills |
| REQ-AP-006 | Plugin MCP Servers | Not started | `<plugin>.<server>` keys merged after user config; full `${PLUGIN_*}` env contract |
| REQ-AP-007 | Containment | Not started | Resolved-path containment incl. symlinks for both plugin root and data boundaries |
| REQ-AP-008 | Enable and Disable | Not started | Persisted registry (SQLite columns, `mcp_disabled_servers` precedent); absent record = enabled |
| REQ-AP-009 | Add and Remove Lifecycle | Not started | Local path or git URL add; unmanaged-provenance protection; data retained on remove, explicit purge |
| REQ-AP-010 | Status Surfacing | Not started | Per-plugin per-component load outcomes and missing-root state |
| REQ-AP-011 | Extension Namespaces Reserved but Inert | Not started | `dev.scottopell.phoenix-ide/` reserved; all namespaces ignored in v1 |
| REQ-AP-012 | Version Recognition | Not started | Exact canonical 1.1.0 identifiers; mixed-version packages disable MCP contribution |

## Planned implementation phases

1. `crates/phoenix-plugins` — pure manifest/mcp.json parsing, failure
   boundaries, placeholder expansion, env overlay, containment checks; unit
   tests from the specification's own examples.
2. MCP runtime integration — `cwd` on `McpServerConfig::Stdio`, transport
   translation, plugin servers merged after user config in
   `read_all_configs`, `PLUGIN_DATA` lifecycle.
3. Skills integration — plugin `skills/` source in discovery, namespacing,
   precedence, catalog provenance marker.
4. Lifecycle UX — SQLite registry, `/api/plugins` routes, Plugins page,
   `phoenix-client.py` subcommands.

## Cross-Spec References

- `specs/skills/` — discovery precedence, turn-scoped catalog advertisement
  (plugin skills ride REQ-SK-006 precedence and REQ-SK-009 refresh).
- `specs/mcp/` — transport-tagged config discovery and first-seen-wins merge
  (plugin servers extend REQ-MCP-001), live tool resolution (REQ-MCP-014),
  reload reconciliation (REQ-MCP-015), enable/disable persistence precedent
  (REQ-MCP-016).
