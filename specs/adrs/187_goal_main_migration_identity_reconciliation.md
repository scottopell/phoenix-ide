# ADR-187: Preserve main migration identities when extracting the Goals schema

- Status: Accepted
- Date: 2026-10-27
- Requirements: REQ-COMP-020

## Context

The unshipped source stack behind PR #975 assigned its nine schema entries to 130–138, overlapping main's continuation-handoff and request-capacity migrations at 130/131. The bounded reconciliation was authorized on October 26. The user subsequently requested a coherent ordered extraction queue rather than landing the combined runtime stack. Schema identity is shared by those later slices and cannot be independently renumbered in each one.

## Decision

Retain main's existing 130/131 registry entries and SQL unchanged. Register the source suffix once at 132–140, retaining its reconciled names, SQL, and migration helpers. Include the remote-origin and terminal-subscription schema dependencies, but not peer runtime, handlers, pollers, or activation. Goal repositories and execution follow separately.

Reject unsupported source-branch and unknown ledger identities before either bootstrap or numbered migration writes. Use read-only evidence from canonical names, contiguous post-129 entries, marker columns/types, introduced table groups, and the paired local-Stop triggers to reject partial, orphaned, mixed, or ambiguous lineages. Do not convert, relabel, reset, or infer provenance for an unshipped branch database.

Preserve existing main writers under the new constraints. In particular, canonical turn materialization must set the schema's materialization witness in the same update as the canonical-message identity. Keep the actual repository-path regression, not only a synthetic SQL fixture.

## Consequences and verification

The first extracted PR contains the schema prerequisite and bounded existing-writer compatibility only. Qualification must prove SQL/identity preservation, unchanged refusal through both migration entrypoints, supported canonical prefixes and idempotence, and the actual local materialization/reference/watch paths. Results from the combined tree do not qualify the extracted candidate.

This decision grants no deployment interval, downgrade, mixed-version operation, automatic conversion, or live-database operation. Historical launchd qualification pins remain unchanged. Normal CI and independent review remain landing gates.
