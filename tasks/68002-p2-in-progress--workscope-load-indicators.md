# Implement WorkScope load indicators

User requested this completed design be added to the implementation queue on 2026-10-09. Queue as ready; do not start automatically or preempt active delivery. No merge/deployment approval is implied.

Design owner and source: @conv:2dd783fa-c469-4510-9ac0-a29af03cd2e5
Final baseline: @transcript:96ccb8a6-5bd1-4a36-a636-ffc879645790#message-0d256e9d-11ac-4daf-b19d-886ba9d295b5
User tool/API decision: @transcript:96ccb8a6-5bd1-4a36-a636-ffc879645790#message-96ccb8a6-5bd1-4a36-a636-ffc879645790:dc7aa0bd-0a71-4d2d-8121-4cfe639723c4

## Agreed scope
- First-party API for one WorkScope and a breakdown across WorkScopes, plus a Global-Coordinator-only tool. Both consume the same underlying resource summary.
- WorkScope-wide accounting regardless of initiating conversation: Global-launched jobs count under their target scope. Deduplicate attributed processes; no second coordinator total.
- Existing owned bash process groups define attribution. Detached descendants are excluded; make measurement boundaries explicit.
- Always-on CPU observations while tracked jobs are live, targeting roughly five-second intervals with resource-aware collection.
- CPU as cores consumed: latest interval and 1/5/15-minute averages with actual available history coverage. History is in memory only; server restart resets coverage.
- Fresh shared demand-driven memory samples, retaining platform semantics and explicit unavailable values.
- Small inline UI indicators, not a dedicated page or modal.
- Reuse existing collection infrastructure, not a new monitoring framework.

## Explicit exclusions
Disk measurement/caching, run-queue metrics, host diagnostics, reclamation, automatic resource controls, persistence/recovery for CPU history, and automatic conversation-context injection.

## Implementation and acceptance
Resolve exact API/tool shapes, API access rules and inline placement within this baseline. Preserve ownership versus control distinctions. Test shared-scope/coordinator attribution and deduplication, CPU windows with partial coverage and restart reset, demand-driven memory/unavailable semantics, and Global-only tool exposure. Verify inline indicators in the actual UI. Do not broaden to host monitoring or disk cleanup because those motivated the original discussion.

Retain the design conversation as the implementation handoff; coordinate scheduling through the existing project delivery queue and roadmap #806.
