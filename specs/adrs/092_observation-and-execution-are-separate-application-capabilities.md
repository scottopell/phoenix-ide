# ADR-092: Observation and execution are separate application capabilities

- **Status:** Accepted
- **Date:** 2026-10-11
- **Related requirements:** REQ-API-007, REQ-CONV-006, REQ-BT-019

## Context

A conversation-load investigation required replaying a large persisted conversation without enabling recovery, providers, tools or worktree mutation. A separate read-only fixture server reused snapshot SQL but supplied fixed auxiliary API responses. That let a targeted browser pilot run, but exposed an apparently general application with incomplete navigation and divergent initialization behavior.

The user rejected expanding that parallel server and approved a target-only replay through the normal binary, route table and UI. They endorsed: “Partial capability coverage is acceptable. Parallel application behavior is not,” and “Observation does not imply activation.” They also identified enforceable code guardrails as important because LLMs implement Phoenix changes.

The existing AppState bundles observations and execution services; its construction and earlier startup perform effects. Removing credentials or denying writes after construction does not prevent those effects. Stream initialization also combines persisted observation with runtime materialization and possible persisted normalization.

## Decision

Compose shared observation services and execution authority separately. Select a replay composition before constructing live services or entering startup recovery. Keep the normal application entry point and route registrations. Reuse production read/projection behavior rather than manufacturing alternate successful HTTP responses.

Introduce bounded observation services incrementally when needed by a verified journey. Do not make every AppState field optional, introduce a giant universal IO facade, or undertake a repository-wide refactor before the target replay works. Preserve the existing live bundle while establishing fallible request admission and narrowing dependencies.

Read-only replay names one composition; caller behavior is governed by specific capability availability, not a global mode boolean. Expected absence is explicit and ordinary. Commands do not report success without performing the effect promised by their contract. Observation and activation are distinct responsibilities even if normal UI orchestration invokes both.

Use types and narrow ast-grep rules to protect each extracted boundary. These are architectural guardrails, not a security sandbox. Server admission is independent of UI affordances, and immutable persistence supplements rather than replaces authority separation.

The initial replay is a private same-host development composition, not a remote unauthenticated application or a schema compatibility promise. The provisional observation host accepts direct loopback peers without forwarding headers; forwarded, remote and missing-peer requests are rejected. It does not construct a persistent session store. Normal auth retains its existing semantics. Future authenticated/remote replay requires an explicit contract rather than expanding this policy incidentally.

The mock-provider composition is separate future work: writable isolated persistence, real command admission and explicit execution adapters would be required. Continuation, recovery and tool execution are excluded from this replay scope.

## Alternatives

- **Expand the fixed-response server:** rejected because it duplicates application behavior and can fabricate readiness or successful service availability.
- **Global read-only boolean and optional AppState fields:** rejected because authority remains constructed and enforcement scatters across call sites.
- **Universal runtime/IO wrapper now:** deferred; long-term capability-specific services have value, but a broad rewrite is not earned by this target journey.
- **OS sandbox alone:** insufficient for coherent behavior when effects are denied. Nono is a possible later defense-in-depth evaluation, not part of this implementation or an established guarantee.

## Consequences

- Shared read behavior and explicit admission improve replay fidelity and future testability.
- A mechanically broad handler-extractor migration is needed to retain one route table without hidden live fallthrough.
- Supported coverage is defined by validated journeys, not by the number of rows in the fixture.
- Process-local stream evidence and persisted snapshots must be distinguished; an immutable DB does not reconstruct in-flight events.
- Replay and production costs require calibration before baseline statistics or optimization claims.
- This does not retire producerless durable-wake infrastructure or redefine async handle semantics.

## References

- `specs/api/requirements.md`
- `specs/conversation-ui/requirements.md`
- `specs/sse_wire/sse_wire.allium`
- `specs/auth/requirements.md`
- `specs/compatibility/requirements.md`
- `docs/conversation-replay-dependency-map.md`
- `ConversationRouteObservations`, `ApplicationState`, `LiveState`, `stream_conversation`
