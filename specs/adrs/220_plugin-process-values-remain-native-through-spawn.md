# ADR-220: Plugin process values remain native through spawn

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-AP-006, REQ-AP-009, REQ-MCP-003

## Context

Lossless registry paths did not prevent a second loss at subprocess admission:
resolved package commands and client-owned path environment values were converted
to Unicode strings. A valid non-Unicode package path could therefore pass
containment yet name a different executable at spawn. The same issue applies to
path placeholders inserted into arguments or configured environment values.

## Decision

Use native OS strings for resolved commands, arguments and environment overlays,
then carry those types through MCP configuration, supervisor comparison, initial
spawn and respawn. JSON configuration remains Unicode at the parsing boundary;
conversion into native values is lossless. Placeholder expansion scans the
original configured text once and appends native path units without re-scanning
inserted values. No display conversion participates in execution.

Before data purge, repeat installation-entry and same-name package absence checks
after asynchronous reconciliation. A package restored during that wait blocks
data deletion and preserves registry identity. This complements the entry
identity revalidation recorded in ADR-219.

## Options considered

Rejecting all non-Unicode home paths would impose a new filesystem restriction.
Keeping String and OsString copies simultaneously would permit divergence.
A separate plugin-only spawn transport would duplicate execution and recovery
behavior. A single native process representation removes the lossy boundary
without another execution path or activation cache.

## Consequences

MCP stdio configuration constructors accept native values; HTTP configuration
and public JSON shapes are unchanged. Native values participate directly in
reload equality. Tests use native values rather than weakening their assertions.
Linux covers actual raw-name executable launch; Unix pure expansion tests cover
exact units independently of filesystem restrictions. Late absence checks do
not promise an atomic transaction across a concurrently modified shared root,
SQLite and the runtime.
