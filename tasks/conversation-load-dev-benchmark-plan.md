# Establish an isolated, representative conversation-load development benchmark

## Goal
Create a local development loop on essmbp using the existing Phoenix browser performance tools. Establish and retain a representative slow-load baseline before any optimization. Do not substitute filing this plan for executing the approved work.

## Evidence
User Safari capture: conversation-list click to missing transcript appearing about eight seconds later, through pp.bigpartydipper.party to essmbp production. Local Chromium pilots against localhost:8031 reproduced about 3.67 s and 3.49 s to transcript readiness proxies. First product-conversation request waited approximately 2.68 s and 2.46 s for first byte; repeated requests and a subsequent approximately 455 KB snapshot precede transcript display. One duplicate spent approximately 2.44 s before requestStart, which is not server execution. Browser long tasks do not account for the multi-second delay. Target product conversation is 7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f; current member observed was bf738a43-e882-4b3b-a74e-8c8cb56d0e63. Production asset observed: index-GOlpszya.js. Initial production identity supplied by user: v0.13.0-rc.3, c5c33a53ccba318b6b6fb571dedb577703a63fac; verify rather than assume unchanged.

## Scope and sequence
1. Read development, task-tracking, compatibility, relevant conversation specs and roadmap context. Verify isolated worktree ownership and preserve other owners' work. If GitHub unavailable, report inability to verify roadmap.
2. Inspect existing dev.py launch/configuration, fixture/seed facilities, conversation-open telemetry, request producers/consumers, and safe runtime isolation options. Use bounded production traces or metadata queries only as needed to identify what scale/work the fixture must preserve. Do not optimize or deduplicate requests yet.
3. Design and enforce fixture effect isolation BEFORE starting a server on production-derived data: no active providers/tools/chain/coordinator/background execution, no production worktree or filesystem writes, distinct data/auth/session/config paths and ports, no imported secrets. Do not assume disabling provider credentials alone prevents effects. If safe isolation cannot be demonstrated, stop before cloning/startup and explain the blocker.
4. Prefer a minimized relationally valid sanitized fixture preserving causal scale: continuation/member relationships, message and content lengths, tool-card mix, transcript geometry, and conversation-list/global scale if they contribute. A tiny synthetic conversation is not representative. If extraction needs a consistent private intermediate, use SQLite online backup or equivalent consistent snapshot after estimating disk capacity; keep intermediates private, ignored and local, strip credentials/private values before use, and document lifecycle. Never export private messages or secrets, raw HARs or screenshots. No live DB downgrade or migration. Historical build/data comparisons must use compatible isolated data.
5. Start development through ./dev.py with the isolated fixture and safe configuration. Confirm frontend/backend build identity and compare release/debug overhead; do not artificially add sleeps to manufacture slow behavior.
6. Reuse browser_profile.run_scenario rather than adding a benchmark framework. Keep setup/list readiness outside the measured interval; automate list click, identify correct product/member, require visible meaningful transcript, record frame-based readiness proxy separately from actual correctness, verify enabled input without sending, exercise scrolling and check intended anchor. Capture sanitized resource start/requestStart/responseStart/end, payload sizes, duplicate counts, and browser long tasks. SSE duration zero is not evidence of instantaneous readiness. Do not sum overlapping intervals.
7. Run one development pilot, inspect correctness and attribution against the production shape. Refine data/setup until the analogous bottleneck is represented, or explicitly report why it cannot yet be reproduced. Then agree a bounded repeated-run count and budgets with the user before establishing the benchmark sample set; retain individual samples, environment/build/data identity and observer overhead. No significance claims from two exploratory samples.
8. Persist a small replayable scenario/config and timing-only reporting via existing tooling, plus fixture setup/isolation instructions and validation tests where necessary. Deliver commands/steps for resetting and rerunning the development loop. Commit completed scoped units after validation. Report results in this conversation with evidence, limitations and next optimization candidates; optimization is a subsequent scope.

## Acceptance
- A working isolated local dev server and repeatable browser-tool list-click scenario, not only a proposed task.
- Representative slow-load starting point with matching request/data/readiness evidence, or a precise blocker demonstrated rather than a misleading synthetic baseline.
- Correct transcript/member and usable scrolling/composer/anchor verified; readiness proxies named honestly.
- Raw per-run timing samples retained without private content; reproducible fixture identity and reset methodology documented.
- Isolation demonstrably excludes active effects and production filesystem/data/config mutation.

## Non-goals
No optimization, request deduplication, broad cleanup, new benchmark framework, release gate, PWA/deep-link blocker changes, remote devmbp execution, deployment or production restart. No clearing user storage, editing auth/TLS/keychain, staging/resetting other owners' changes, or sharing production-derived private artifacts.

## Approval boundary
Approval authorizes isolated development benchmark implementation and necessary safe local fixture preparation, subject to the isolation constraints above. It does not authorize starting an unsafe cloned runtime or changing live production.
