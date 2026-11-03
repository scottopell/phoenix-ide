# ADR-221: Plugin failures and status use explicit source scope

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-AP-006, REQ-AP-009, REQ-AP-010, REQ-MCP-015

## Context

Dotted server keys do not prove ownership: a user key or another plugin can
share the same prefix. Plugin control incorrectly treated such failures as its
own. Global skill status also used HOME as a working directory, which pulled
unrelated child projects into deduplication. A dangling installation link exposed
another distinction: the package target can be missing while its entry remains.

## Decision

Tag MCP configuration with a client-generated user/plugin source at discovery.
Carry that tag with serving and queued configuration. Failure diagnostics contain
a unique owner set derived from the requested configuration and configurations
still retained or queued at failure time. Plugin control matches exact identities,
not name prefixes. No separate mutable ownership cache is introduced. Ownership
changes participate in reconciliation so an ownership handoff cannot silently
reuse an old attribution. An orphan credential cleanup has no known actor source
to infer; its global diagnostic remains visible without a fabricated owner.
Destructive plugin retirement also checks exact serving/queued bindings after
reconciliation. A deferred actor removal is not completion: retain package files
and data, report pending retirement, and permit an explicit retry. Unrelated
user or plugin bindings do not block retirement by prefix coincidence.

Evaluate global plugin skill status against global user, builtin and plugin
sources only. Conversation discovery still applies repository/projects-directory
precedence to its actual working directory. Global status is not a promise that
a skill wins every conversation's catalog.

Report package-target missingness separately from installation-entry state.
The latter is present, absent or unknown. A dangling entry is shown as missing
but offers Remove, not Purge. Destructive endpoints still revalidate the live
filesystem; UI metadata is not deletion authority.

## Options considered

Splitting dotted keys cannot recover their origin. A side map parallel to server
state could diverge from serving or queued configuration. Keeping the source in
the configuration makes propagation explicit and failure attribution testable.
A sentinel working directory would merely replace one arbitrary skill context
with another; an explicit context-free discovery mode expresses the global scope.
Using one missing boolean for both target and entry conflates distinct facts.

## Consequences

Reload diagnostics add owner metadata; UI types carry it. Static Authorization
in portable plugin headers is rejected at admission because the HTTP transport
reserves that header for its auth credential model. Users can configure a static
credential in user MCP configuration. This is an explicit capability gap, not a
new plugin auth flow. Existing CLI platform actions compose without an early
plugin-only exit. No cross-harness transaction or new recovery promise is added.
