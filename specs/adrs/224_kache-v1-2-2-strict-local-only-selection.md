# ADR-224: Select Kache v1.2.2 only behind a Phoenix-owned local-only boundary

- **Status:** Accepted
- **Date:** 2026-11-01
- **Supersedes:** Selection conclusion of ADR-223 only; historical v1.0.0 evidence in ADR-222 and ADR-223 remains applicable to its release.
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-008

## Context

[ADR-222](222_kache-v1-restores-automatic-selection.md) established representative macOS arm64 restored-archive debugger fidelity for Kache v1.0.0, not for every later release. [ADR-223](223_kache-default-waits-for-identifiable-isolation-fix.md) blocked default selection of official v1.0.0: its remote-enabled trust-domain behavior could fail open and leave configured volume shards unscoped, while the corrected private candidate reported the same version as the vulnerable release. Its conclusion assumed the managed-remote configuration surface was part of selection. A bounded local-only milestone can exclude that surface instead of treating remote qualification as a prerequisite.

The published Kache v1.2.2 tag peels to commit `96052b4842239bb216f6c4812054203c21550788`. Its macOS arm64 release archive (`kache-aarch64-apple-darwin.tar.gz`) has SHA-256 `10d09e146852a8df8796cf642ca58872107a7f391b7647ec8cc253cf289af0a9`. Native qualification of that artifact used isolated test-owned paths and a hostile ambient remote configuration. Phoenix selected its owned local-only config and child; a second clean source root restored both application and dependency compilations as local hits without additional misses. Kache restored a UUID-matched dSYM, and LLDB resolved and stopped at application and dependency file/line breakpoints. The journey did not read active credentials, contact remote storage, reuse a live daemon, or inspect or mutate existing cache or upload-queue state.

## Options considered

1. **Keep Kache disabled until managed-remote isolation and native debugger fidelity are qualified for a new release** — preserves ADR-223's broad gate but prevents a bounded local-only use case from advancing.
2. **Select v1.2.2 under Phoenix-owned strict local-only configuration** — excludes the remote trust-domain surface while retaining explicit daemon ownership and honest qualification limits.
3. **Select v1.2.2 with ambient or remote-capable configuration** — allows remote behavior without its own isolation and qualification contract.

## Decision

Phoenix selects exact Kache `1.2.2` on macOS arm64 for **local-only** compiler caching. Automatic selection prefers it when the owned daemon becomes ready, otherwise falls through to usable sccache or no cache and reports the actual backend. Explicit `kache` selection fails actionably rather than silently choosing another backend. Other Kache versions, including official v1.0.0, remain ineligible for this selection contract.

Phoenix generates a private config and explicitly sets **both** `KACHE_CONFIG` and `KACHE_HOST_CONFIG` to it. The config sets `[cache] local_only = true` and `ignore_env = true`, plus explicit Phoenix-owned `local_store` and `runtime_dir`; Phoenix configures no remote backend, planner, or volume. Ambient Kache remote settings and credentials cannot activate remote behavior. Phoenix owns the selected socket, receipt, and child lifecycle: readiness and reuse require the serving-process and socket-incarnation ownership proof; an unowned or replaced endpoint fails closed. Phoenix never stops or restarts an existing daemon. Generated cache settings are scoped to the Cargo builds Phoenix owns, not the server or agents' commands.

Remote caching is **deferred, not obsolete**. It requires a separate explicit contract and qualification before it can be enabled. Historical v1.0.0 trust-domain failure and private fork correction remain relevant evidence for that separate work, but no longer block this strictly local v1.2.2 selection.

## Consequences

- A published versioned release can be selected by default without accepting ambient Kache remote policy or relying on an indistinguishable private fork.
- The local-only selection is backed by release-specific native restored-archive and source-level debugger evidence; it is not a production deployment or remote-cache qualification.
- No remote-cache compatibility, performance, cross-version cache compatibility, or broader-platform guarantee follows from local selection.

## References

- [REQ-COMP-008](../compatibility/requirements.md)
- [Compiler cache development guide](../../docs/development/compiler-cache.md)
- [ADR-222](222_kache-v1-restores-automatic-selection.md) and [ADR-223](223_kache-default-waits-for-identifiable-isolation-fix.md)
