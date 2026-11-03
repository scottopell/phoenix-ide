# User-Level Agent Plugins — Executive Summary

## Scope

Phoenix consumes Agent Plugins 1.1.0 bundles from immediate user-level entries
under `~/.agents/plugins/`. Skills use `plugin:skill`; MCP servers use
`plugin.server` keys after user configuration. The reserved `data/` subtree
holds retained state. A plugin named `data` uses a separate installation entry.
No repository-based plugin discovery or extension execution is added.

PR #1017 is open for review. Task 98021 remains in progress. Review readiness
and passing CI do not imply merge, deployment, or full browser/runtime acceptance.

## Implementation and coverage

| Requirement | Implementation and verification |
|---|---|
| REQ-AP-001 | Home-derived user root; installation entries remain distinct from canonical package targets |
| REQ-AP-002 | One-level deterministic scan, manifest identity, duplicate first-wins; regular-file admission rejects FIFO manifests without reading |
| REQ-AP-003 | Offline manual manifest validation with typed field checks and report/ignore rules |
| REQ-AP-004 | Per-skill/server/component outcomes; closed MCP document, header syntax validation, special-component diagnostics |
| REQ-AP-005 | Ordinary namespaced skills share injected activation across prompt, invocation and initial/later expansion; global status excludes arbitrary project contexts |
| REQ-AP-006 | Native OS command/argument/env values through spawn/respawn; one-pass expansion; approved resolved cwd; writable data probe; origin-confined HTTP redirects; unsupported plugin Authorization headers are skipped |
| REQ-AP-007 | Existing symlink ancestors resolved even for missing descendants; package execution and client-owned data boundaries remain separate |
| REQ-AP-008 | SQLite authority plus one instance activation snapshot; failed initial read excludes plugins, failed refresh preserves last-good state; serialized capture/reconciliation; exact failure owners, not dotted-prefix inference |
| REQ-AP-009 | Exclusive staged add, Git-supported source forms, entry-safe remove, retained disabled identity, lossless native BLOB paths; identity/absence rechecks after teardown preserve replacements and restored packages |
| REQ-AP-010 | Structured global admission outcomes; user/cross-plugin collision reasons; source-owned errors; target missingness separate from present/absent/unknown entry state; settings navigation and retained controls |
| REQ-AP-011 | Extension namespaces remain inert |
| REQ-AP-012 | Exact 1.1.0 schema recognition; incompatible MCP document disables only that component |

Global skill status includes user/builtin/plugin precedence, not a promise that
a plugin skill wins every conversation's repository-specific catalog. MCP
`loaded` means offline admission, not connection health. Reload failures carry
exact desired and retained/queued owners. Orphan credential cleanup without
configuration has no fabricated plugin attribution.

A dangling entry has a missing target but remains removable, not purgeable.
Unknown entry state does not authorize destructive actions. Registry provenance
belongs to the selected installation entry, not every same-name sibling.

## Verification

The hosted CI workflow passed on `b67ec1d48` and earlier repair checkpoints.
Later review edits require their own exact-head run; do not substitute those
older green results. Current focused evidence: 41 loader, 212 MCP, 66 skills,
46 macOS runtime/API and 17 Python client tests; strict clippy, workspace compile,
and UI/spec/structural checks. Suite totals include existing tests.

Linux tests cover raw-name filesystem lifecycle and bundled executable launch;
Unix pure tests cover native units independently of filesystem restrictions.
macOS rejects creating some invalid filename byte sequences. Registry byte
round-trips are tested separately from that restriction. Disposable lifecycle
and Axum tests are not real success-path browser/MCP mutation acceptance.
Earlier broad local macOS fixture failures remain recorded in the task rather
than being labeled baseline-clean.

## Limits and decisions

The numbered migration covers fresh and supported pre-plugin databases. Earlier
unshipped prototype schemas are preserved, not automatically converted. Native
path storage adds no cross-OS database portability promise. Retirement checks
close known asynchronous gaps; they are not an atomic filesystem sandbox or a
transaction across independent harnesses, SQLite and runtime resources.

A failed removal can leave a disabled identity and intact entry. A genuinely
absent shared root has no data to delete and is not recreated by purge. A
dangling root/ancestor is not proof of absence. Partial failures are reported;
no automatic rollback or new crash-recovery guarantee is claimed.

ADRs [216](../adrs/216_agent-plugins-are-user-level-shared-root-namespaced-bundles.md),
[217](../adrs/217_plugin-lifecycle-separates-installation-ownership-from-activation.md),
[218](../adrs/218_mcp-redirects-preserve-origin-and-reload-capture-is-serialized.md),
[219](../adrs/219_plugin-removal-revalidates-entry-identity-and-retains-native-paths.md),
[220](../adrs/220_plugin-process-values-remain-native-through-spawn.md), and
[221](../adrs/221_plugin-failures-and-status-use-explicit-source-scope.md) record
the decisions. Skill precedence/advertisement and MCP transport/reconciliation
contracts remain in `specs/skills/` and `specs/mcp/`; compatibility policy remains
in `specs/compatibility/`.
