# WorkScope resource visibility

## Purpose

Agents and users need to understand how busy a WorkScope is and its attributed memory consumption. Resource accounting follows ownership, not the conversation that initiated a job. Host diagnostics remain the responsibility of existing host tools.

## REQ-WSLOAD-001: Attribution

THE SYSTEM SHALL account for live Phoenix-owned bash process groups under their owning WorkScope, including Coordinator jobs targeting that scope. THE SYSTEM SHALL deduplicate native process identities within a scope. WHEN ownership conflicts across scopes, THE SYSTEM SHALL report affected totals as unavailable rather than count twice or assign by arbitrary ordering.

THE SYSTEM SHALL exclude descendants that detach from the owned process group, terminal/browser/shared-server resources outside those groups, and disk usage. Process identity validation SHALL require the observed group to contain the exact launch leader PID and start-time incarnation before attributing any of its members. An absent or reused leader SHALL make the affected scope's current CPU and memory totals unavailable even if other group members remain; the system SHALL NOT adopt those members as a replacement launch proof. Missing or incomplete observations SHALL NOT become authoritative partial totals.

## REQ-WSLOAD-002: CPU collection and history

WHILE tracked bash jobs are live, THE SYSTEM SHALL collect consumed CPU using an approximately five-second cadence, without requiring a query. WHILE no tracked jobs are live, THE SYSTEM SHALL perform no native CPU sampling for this history. Background collection SHALL NOT sample memory or host metrics.

THE SYSTEM SHALL expose latest-interval CPU and time-weighted one-, five-, and fifteen-minute averages as cores consumed, where one core represents one fully occupied CPU core. Each window SHALL include actual measured coverage in seconds. Incomplete intervals, process membership changes, counter regressions, and identity changes SHALL contribute no fabricated coverage. A window with no measured coverage SHALL report an unavailable average. A scope with no live jobs SHALL report zero current consumption while retaining unexpired measured history.

History SHALL be bounded to the rolling fifteen-minute horizon and held only in server memory. Restart SHALL reset coverage. CPU sample freshness SHALL be exposed separately from demand-sampled memory freshness.

## REQ-WSLOAD-003: Demand memory

WHEN a resource summary is requested for live jobs, THE SYSTEM SHALL obtain fresh memory through the shared demand-driven observation lease defined in ../deployment-info/requirements.md REQ-DEPLOY-007a. Concurrent requests SHALL reuse fresh observations; an across-scope response SHALL share one observation generation. Memory SHALL preserve platform-specific proportional/footprint semantics and unavailable values. A scope without live jobs SHALL report zero current memory and process count.

## REQ-WSLOAD-004: API and tool admission

THE SYSTEM SHALL expose an API for an individual WorkScope and for a breakdown across active persisted WorkScopes. An ordinary conversation SHALL be admitted only to its attached WorkScope. Across-scope queries and queries outside the attached scope SHALL require the current Global Coordinator identity, checked server-side through existing authority resolution.

THE SYSTEM SHALL expose a resource-summary tool only in the Global Coordinator tool registry and SHALL recheck the current Coordinator identity at execution. API and tool SHALL consume the same summary service. THE SYSTEM SHALL NOT inject summaries automatically into conversation context.

## REQ-WSLOAD-005: Inline presentation

THE SYSTEM SHALL show compact CPU and memory information inline in the existing WorkScope resource section, without a dedicated page or modal. Coverage and freshness SHALL be available through progressive disclosure. Unavailable values SHALL remain distinct from zero; stale or failed current measurements SHALL NOT appear current.

The UI SHALL enable collection only from a confirmed live, nonarchived conversation surface with a matching attached WorkScope inventory. Replay or fixture surfaces without this capability SHALL remain inert. Polling SHALL avoid overlapping requests and discard responses after scope or conversation changes.

## Exclusions

No disk collection/cache, run-queue augmentation, general host diagnostics, reclamation advice/actions, scheduling/resource controls, durable CPU history, or new monitoring framework.
