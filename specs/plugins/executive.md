# User-Level Agent Plugins — Executive Summary

## Overview

Phoenix consumes Agent Plugins 1.1.0 bundles from user-level entries under
`~/.agents/plugins/`. Skills use `plugin:skill` names; MCP servers use
`plugin.server` keys after user configuration. The reserved `data/` subtree
holds retained state. A plugin named `data` uses a separate install entry.

[ADR-216](../adrs/216_agent-plugins-are-user-level-shared-root-namespaced-bundles.md)
records scope and naming.
[ADR-217](../adrs/217_plugin-lifecycle-separates-installation-ownership-from-activation.md)
records ownership, activation, publication, and failure boundaries.

## Status

Implementation is in PR #1017. The task remains in progress. Follow-up
review fixes cover serialized reload capture, same-origin HTTP redirects,
and parsed loopback hosts. Review readiness is not merge, deployment, or
complete runtime acceptance. The earlier completion table overstated
lifecycle and status coverage.

| Requirement | Implementation | Verification and limits |
|---|---|---|
| REQ-AP-001 | Home-derived, user-level discovery only | Installation entries, not canonical symlink targets, define discovery scope |
| REQ-AP-002 | One-level scan, manifest-name identity, first duplicate wins | Invalid/missing manifests do not establish installation-entry absence |
| REQ-AP-003 | Offline manual manifest validator | Exact schema recognition, structural validation, report-and-ignore semantics have loader tests |
| REQ-AP-004 | Parsed candidates with scoped failure reports | Filesystem, skill semantic/catalog, and MCP preparation failures remain separate |
| REQ-AP-005 | Namespaced ordinary skills through instance-owned `SkillDiscovery` | Advertisement, slash expansion, and invocation receive shared activation; initial-message and dedup regressions added |
| REQ-AP-006 | Shared MCP preparation adapter | Selected-anchor cwd, single-pass expansion, package command resolution, data creation; HTTP origin policy rejects cross-origin redirects and bounds same-origin redirects; parsed IPv4/IPv6 loopback classification |
| REQ-AP-007 | Manifest/component/executable containment | Existing symlink ancestors are resolved even for nonexistent descendants; install entry and execution root remain distinct |
| REQ-AP-008 | SQLite authority plus one instance-scoped `PluginActivation` | Initially unavailable excludes all plugins; failed refresh preserves last good snapshot; startup/reload capture and reconcile under one lock, with no pre-lock desired snapshot |
| REQ-AP-009 | Exclusive staged add; entry-safe remove; retained disabled identity; guarded purge | Real filesystem/SQLite and Axum regressions exist. Cross-harness identity locking and automatic partial-failure rollback are not promised |
| REQ-AP-010 | Tagged loaded/skipped/disabled item outcomes and reasons | Skill status uses catalog admission; MCP status uses offline preparation, not live connection health. Registered invalid/unknown entries expose `load_error`, not false missing status |
| REQ-AP-011 | Extension namespaces inert | No extension execution path |
| REQ-AP-012 | Exact 1.1.0 schema identifiers | Unsupported manifest version rejects; incompatible MCP document disables that component |

## Late-review ownership and format checks

A later review on the CI-green checkpoint found eight more gaps. Removal now
captures Unix device/inode identity before teardown and revalidates it and the
manifest name immediately before deletion. Registered fallbacks declaring
another valid plugin are refused. Registry installation paths are typed native
paths stored as BLOB values; presentation strings do not confer deletion
authority. [ADR-219](../adrs/219_plugin-removal-revalidates-entry-identity-and-retains-native-paths.md)
records these boundaries and their limits.

MCP admission detects qualified-name collisions across plugins in one pass,
shared with status. Headers are syntax-validated and the MCP document is closed
to unknown top-level fields. Manifest metadata must identify a regular file
before reading, so FIFO siblings are skipped. Git validates all nonempty
repository source forms after the option terminator, including SSH and scp-style
sources without a `.git` suffix.

The earlier checkpoint passed the complete hosted CI workflow. That result does
not cover subsequent late-review edits. Focused tests for the new edits pass:
38 loader, 210 MCP, 43 runtime/API on macOS, and five registry tests. The raw
non-Unicode filesystem lifecycle regression is Linux-specific; macOS storage
round-trip tests cover the byte-safe registry separately. New-head CI remains
a separate verification gate until it completes.

## Outstanding-thread repairs

Selected-entry provenance now requires a matching stored managed entry;
observing another same-name entry records unmanaged provenance. MCP status and
execution share user-config collision decisions. Child processes inherit the OS
environment without enumerating it as Unicode, and data admission performs a
create/write/remove probe before launch. Empty stdio commands are rejected by
the package validator. Settings includes a Plugins navigation button.

Lifecycle controls reject unknown identities, retain known enable/disable
intent, and allow removal using a validated stored entry when its manifest is
invalid or its target is dangling. Each destructive target remains an immediate
non-data child of the trusted root. Tests cover unsafe stored paths and preserve
outside witnesses.

CI-specific repairs replace `/root` test paths with isolated temporary roots,
include migration 150 in the remaining image-retirement counts, and extract the
prompt's production source before its test module rather than its first
test-only item. These are verification repairs, not relaxed runtime contracts.

## Follow-up review verification

The follow-up adds a deterministic queued-reload regression, real-socket
cross-origin rejection/same-origin preservation/redirect-limit regressions,
and parsed IPv4/IPv6 loopback tests. Loader suite: 34 passed. MCP suite:
205 passed. A separate read-only challenge found no remaining defect in
these three fixes; its sandbox blocked socket execution, so socket-pass
evidence comes from the main test run rather than that reviewer.

[ADR-218](../adrs/218_mcp-redirects-preserve-origin-and-reload-capture-is-serialized.md)
records the shared HTTP transport policy and reload boundary. The redirect
restriction applies to non-plugin MCP servers too; configure the destination
explicitly when an endpoint redirects to another origin.

## Earlier verification boundaries

Focused loader, adapter, registry-upgrade, activation, lifecycle, API, and UI
regressions are the relevant evidence. Existing unrelated MCP/skills tests are
not plugin lifecycle end-to-end tests. Render-only UI tests do not prove real
mutations. The remediation checkpoint passes loader (33), skills (66), MCP
(201), plugin runtime/API (32), registry (4), and strict clippy checks. These
suite totals include existing tests. UI and spec lanes also pass.

The earlier local broad Rust run had six failures. Three migration-related
failures were fixed and passed focused reruns. Three Bash/runtime failures
reproduced alone and remained unclassified against baseline. That checkpoint
had no full-check pass or independent review clearance.

The numbered registry migration covers fresh and supported pre-plugin databases.
It does not convert local databases created by an earlier unshipped prototype
schema. Installation timestamps were removed because they had no consumer.

A failed filesystem removal can leave a disabled retained identity and an entry
still on disk. A malformed installed manifest blocks purge, but Remove can use
the validated retained installation entry without following its package target.
A genuinely absent shared root has no data to delete; purge can forget the
retained identity without recreating the root. Dangling root/ancestor symlinks
do not prove absence. Do not interpret a failed removal as successful uninstall.
Full success-path runtime MCP reconciliation and browser
mutation acceptance remain separate qualification steps.

## Cross-spec references

- `specs/skills/`: skill precedence, deduplication, expansion, invocation, and
  turn-scoped advertisement.
- `specs/mcp/`: configuration precedence, transport execution, live tool
  resolution, and reload reconciliation.
- `specs/compatibility/`: persistence and compatibility policy.
