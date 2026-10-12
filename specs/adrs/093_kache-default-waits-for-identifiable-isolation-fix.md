# ADR-093: Kache default waits for an identifiable isolation fix

- **Status:** Accepted
- **Date:** 2026-10-11
- **Supersedes:** ADR-092
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-008

## Context

ADR-092 qualified released Kache 1.0.0 for restored debug fidelity and selected it automatically on macOS arm64. Managed-remote review then established that the same release fails open to an unscoped primary store when a requested trust-domain directory is unusable and does not scope configured volume shards. Its pre-configuration compiler-family probe cache is also unscoped.

A bounded correction in the user-owned fork makes requested trust domains fail closed and scopes primary stores, configured volume destinations, compiler-family probes, and durable upload queues. Focused executable regressions and independent exact-head review qualify that source candidate. The candidate remains unpublished and reports the same `kache 1.0.0` version as the vulnerable official artifact, so Phoenix cannot distinguish the two through Kache's version contract.

Phoenix can derive one secret-safe trust-domain label from its daemon identity and let corrected Kache apply its own effective configuration. Reimplementing Kache's layered configuration, remote selection, credential use, store resolution, or pull-request socket rules in `dev.py` creates a second configuration engine that cannot remain authoritative.

## Options considered

1. **Continue selecting every binary that reports 1.0.0** — preserves the default but treats vulnerable and corrected binaries as equivalent.
2. **Recognize the unpublished fork by local path or an out-of-band checksum** — makes a private artifact an implicit supported release and creates a machine-local distribution contract.
3. **Disable Kache selection until a corrected published release has a distinct version identity** — preserves honest qualification and keeps the integration ready without claiming the official release is safe.
4. **Replace managed remote use with unconditional local-only mode** — avoids the vulnerable path by changing operator policy rather than fixing the selected backend.

## Decision

Phoenix does not automatically or explicitly select Kache while no corrected published release has a distinct verifiable version identity. Automatic selection falls through to a usable sccache and then no cache while reporting the Kache qualification blocker. Explicit Kache selection fails actionably.

The retained integration derives a path-safe `KACHE_TRUST_DOMAIN` from Phoenix's daemon identity before asking Kache for its effective socket. Corrected Kache owns configuration resolution and isolation of persistent stores, configured volume shards, compiler-family probes, and durable upload queues. Phoenix owns generated socket naming, startup serialization, serving-process proof, socket-incarnation receipts, honest fallback, and secret-safe diagnostics. Phoenix does not stop or restart existing daemons.

A future Kache release may restore automatic and explicit selection only after its distinct published artifact is qualified for both restored debug fidelity and the fail-closed trust-domain contract.

## Consequences

- **Positive:** Phoenix cannot mistake the vulnerable official v1.0.0 artifact for the corrected private candidate.
- **Positive:** Phoenix removes its partial duplicate of Kache's configuration engine.
- **Negative:** Kache acceleration remains unavailable through Phoenix until a corrected release is published and qualified.
- **Neutral:** The local fork candidate and its test evidence remain unpublished; sccache and uncached builds remain available.

## References

- ADR-092, ADR-079, and ADR-078
- `dev.py::_configure_compiler_cache`
- `specs/compatibility/requirements.md`
- `docs/development/compiler-cache.md`
- Local Kache correction `dbdd84955453b45ab13367203c224d8345e5081d`
