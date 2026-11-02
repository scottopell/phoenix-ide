# User-Level Agent Plugins — Executive Summary

## Overview

Phoenix consumes Agent Plugins 1.1.0 bundles from user-level entries under
`~/.agents/plugins/`. Skills use `plugin:skill` names; MCP servers use
`plugin.server` keys after user configuration. The reserved `data/` subtree
holds retained state. A plugin named `data` uses a separate install entry.

[ADR-191](../adrs/191_agent-plugins-are-user-level-shared-root-namespaced-bundles.md)
records scope and naming.
[ADR-192](../adrs/192_plugin-lifecycle-separates-installation-ownership-from-activation.md)
records ownership, activation, publication, and failure boundaries.

## Status

Implementation is under adversarial-review remediation in draft PR #1017.
The task is in progress. No merge, deployment, or complete qualification is
claimed. The earlier completion table overstated lifecycle and status coverage.

| Requirement | Implementation | Verification and limits |
|---|---|---|
| REQ-AP-001 | Home-derived, user-level discovery only | Installation entries, not canonical symlink targets, define discovery scope |
| REQ-AP-002 | One-level scan, manifest-name identity, first duplicate wins | Invalid/missing manifests do not establish installation-entry absence |
| REQ-AP-003 | Offline manual manifest validator | Exact schema recognition, structural validation, report-and-ignore semantics have loader tests |
| REQ-AP-004 | Parsed candidates with scoped failure reports | Filesystem, skill semantic/catalog, and MCP preparation failures remain separate |
| REQ-AP-005 | Namespaced ordinary skills through instance-owned `SkillDiscovery` | Advertisement, slash expansion, and invocation receive shared activation; initial-message and dedup regressions added |
| REQ-AP-006 | Shared MCP preparation adapter | Selected-anchor cwd, single-pass expansion, package command resolution, data creation, HTTP mapping; transport probe awaits a response rather than a sleep |
| REQ-AP-007 | Manifest/component/executable containment | Existing symlink ancestors are resolved even for nonexistent descendants; install entry and execution root remain distinct |
| REQ-AP-008 | SQLite authority plus one instance-scoped `PluginActivation` | Initially unavailable excludes all plugins; failed refresh preserves last good snapshot; no process-global filter or duplicate manager set |
| REQ-AP-009 | Exclusive staged add; entry-safe remove; retained disabled identity; guarded purge | Real filesystem/SQLite and Axum regressions exist. Cross-harness identity locking and automatic partial-failure rollback are not promised |
| REQ-AP-010 | Tagged loaded/skipped/disabled item outcomes and reasons | Skill status uses catalog admission; MCP status uses offline preparation, not live connection health. Registered invalid/unknown entries expose `load_error`, not false missing status |
| REQ-AP-011 | Extension namespaces inert | No extension execution path |
| REQ-AP-012 | Exact 1.1.0 schema identifiers | Unsupported manifest version rejects; incompatible MCP document disables that component |

## Verification boundaries

Focused loader, adapter, registry-upgrade, activation, lifecycle, API, and UI
regressions are the relevant evidence. Existing unrelated MCP/skills tests are
not plugin lifecycle end-to-end tests. Render-only UI tests do not prove real
mutations. The remediation checkpoint passes loader (33), skills (66), MCP
(201), plugin runtime/API (32), registry (4), and strict clippy checks. These
suite totals include existing tests. UI and spec lanes also pass.

The broad Rust run had six failures. Three migration-related failures were
fixed and pass focused reruns. Three Bash/runtime failures reproduce alone
and remain unclassified against baseline. There is no full-check pass or
independent review clearance.

The numbered registry migration covers fresh and supported pre-plugin databases.
It does not convert local databases created by an earlier unshipped prototype
schema. Installation timestamps were removed because they had no consumer.

A failed filesystem removal can leave a disabled retained identity and an entry
still on disk. A malformed installed manifest blocks purge; repair or manual
entry removal is needed before purge can proceed. Do not interpret either case
as successful uninstall. Full success-path runtime MCP reconciliation and browser
mutation acceptance remain separate qualification steps.

## Cross-spec references

- `specs/skills/`: skill precedence, deduplication, expansion, invocation, and
  turn-scoped advertisement.
- `specs/mcp/`: configuration precedence, transport execution, live tool
  resolution, and reload reconciliation.
- `specs/compatibility/`: persistence and compatibility policy.
