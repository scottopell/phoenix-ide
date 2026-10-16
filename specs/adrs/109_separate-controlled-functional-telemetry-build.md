# ADR-109: Separate controlled-functional telemetry build

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-CVT-001, REQ-CVT-005, REQ-CVT-006

## Context

Natural quarter-rate sampling can leave bounded qualification cohorts without positive lifecycle cases. A successful stock run with unsampled departures does not prove sampled pagehide acceptance or cancellation. The user authorized a reviewed, structurally test-only controlled-functional lane while preserving the natural stock baseline separately. Deterministic admission cannot manufacture native resume capability or overhead evidence.

## Decision

Use an owning reporter-factory module: production constructs the unchanged quarter-rate reporter; a separate build-only Vite configuration aliases only the owning hook's factory import to a qualification factory constructing `sampleRate:1`. Both factories use real late-bound fetch. The hook singleton and all lifecycle/outcome/readiness/transport code remain unchanged. A fixed full selection rate truthfully appears as1 in reports and manifests; there is no random-sequence authority, public configuration, query knob or global random override.

The default/release module graph rejects the controlled factory. The controlled build requires the dedicated config/mode/output and exclusive controlled factory graph, clean pinned source and a distinct manifest. Source/config/factory/guard, resolved local source modules and emitted artifact hashes identify the artifact. External dependency provenance is the separately pinned lockfile/environment, not falsely claimed by the local source graph. Manifests live outside asset roots to avoid self-hashing.

Only the cfgtest loopback preview may admit the controlled artifact after independently validating its manifest and hashes. No production server or default bundle switches sampling at runtime. The shared owner supplies that host admission and rejects the controlled evidence class in stock/overhead acceptance. Contract and implementation review precede any execution. A controlled artifact without the host admission is not authorized qualification.

## Evidence classes and finite scope

`stock_natural` remains25% with separate raw evidence; `controlled_functional` is100% and cannot substitute for stock sampling, overhead clearance or full acceptance. Three scenarios per ordinary/Global route produce exactly six expected accepted reports: original retained direct-open owner ready1, pending actual cross-document pagehide incomplete1, and pending real Sidebar-home SPA cancellation1. Every actual report, including unexpected admissions, remains inventory-visible; extra reports fail rather than disappear under a ceiling. The auth-negative case has zero accepted reports. Existing64-journey and retention/privacy/correlation/deadline/normal-drain limits remain unchanged. There is no full stock-driver replay or sampling chase.

Native resume remains unsupported until an actual standalone PWA foreground or persisted BFCache lane is available. Plain desktop tab foregrounding does not satisfy the hook's resume preconditions. Synthetic events, standalone overrides, fabricated BFCache or forced readiness/outcomes are prohibited. No overhead numeric budget is adopted by this decision.

## Alternatives

- Fixed random sequence: adds unnecessary sequence identity/state and misleading quarter-rate semantics; rejected.
- Public/runtime switch or global Math.random patch: leaks a control plane into normal operation; rejected.
- New embedded binary/provider: unnecessary because the existing cfgtest preview admits a separate frontend artifact.
- Repeated stock reruns: chases sampling and does not provide deterministic finite functional evidence; rejected.

## Consequences

The controlled bundle and its gate need independent build/admission checks. Source-level unit tests do not prove actual Vite graph exclusion or native behavior: both artifact graphs must be built/inspected before execution, then the real collector/export/drain oracle must seal every expected ID. The existing green natural baseline remains unchanged; controlled positives cannot retrospectively fill its missing cases.
