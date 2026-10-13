# Correlated conversation-view journey telemetry

## User commission
Global relayed explicit user approval: “Telemetry plan is solid, Commission that work now please, then check in on federation state.” Implement now in one isolated lane, not planning-only backlog. Global retains merge/deploy admission. This is not implicitly a v0.13 release blocker. The iPhone PWA incident immediately before the 12:23 report cannot be reconstructed retrospectively; never claim otherwise.

## Slice 1 — retrievable view experience
Extend existing conversation-open telemetry, not a session recorder. Unique journey/open ID for navigation, reload, Back/Forward and PWA resume, including Global and ordinary routes. Capture shell versus meaningful transcript presentation, chapter-data readiness, breadcrumb presented AND enabled, and anchor settled. Label commit/frame presentation estimates honestly, never as true pixel proof. Primary metric: breadcrumb-ready minus meaningful-transcript, with missing/failed/canceled and potentially reordered marks represented truthfully, not omitted.

Link bounded request/SSE/hydration events, trace IDs, frontend/backend source identities, stable conversation/segment, standalone PWA/coarse platform, visibility/background intervals, reconnect/cache flags, count/size buckets. Monotonic client durations; no browser/server wall-clock subtraction. Include incomplete/canceled/failed/missing-navigation journeys and define their lifecycle and denominators explicitly.

Bounded in-memory buffer, sampling/retention/overhead/drop semantics, best-effort flush only. Never block rendering; no unbounded retries or durable offline queue. No message text, breadcrumb labels, raw URL queries, screenshots or credentials. No per-token/per-frame networking. Optional copy-view-ID only if cheap and not a dependency.

## Slice 2 — evidence-led attribution
As evidence warrants, extend stages for transformation/chapter work/React/layout, feature-detected long tasks/frame delay, and breadcrumb/older-history interactions. No generic performance framework or redesign of existing optimization measurements.

## Qualification
Deterministic tests injecting network versus UI delays, canceled journeys, background/resume, old deep links, continuations and missing events. Demonstrate retrievable correlated trace through existing telemetry backend/collector on Global and ordinary routes, low bounded overhead, correct sampling/drop/failure behavior and absence of forbidden payload data. Exact-head tests/CI and independent cumulative review; distinguish synthetic controls from actual candidate real-server acceptance.

## Ownership and isolation
Base b8fc23f97bf8b6f75a58507c29f720f5c2e32d42, dedicated branch feat/conversation-view-journey-telemetry. Sole telemetry owner owns new journey module/tests and existing open-telemetry/wire/collector seams plus minimal page hooks agreed with history owner stable e93304c8-3830-47e3-8f09-7e1938864d3f and performance owner stable02e7d9c1-80c1-4416-a6e4-3e77788230fe. History owns aggregate/navigation/anchor semantics; perf owns its measurement/SQL experiment. Do not copy either dirty tree, alter history rendering behavior, change perf baselines, or undo Global ordinary-telemetry suppression in PR898 without coordinated correct correlated replacement. Build resources obey host current self-service guard policy; source/light tests can progress independently. No production configuration, collector retention, credential, deployment or unrelated source mutation. No competing ADR allocation: main ends099, history100, federation provisional101, native102; coordinate any additional actual ADR once needed.
