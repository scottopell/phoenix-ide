# Reduce avoidable Bash input failures

## Commission and scheduling
User-approved medium-priority implementation outcome. Task 14003 / PR #803 is complete. The retained Bash tool/orchestration owner is the sole implementation owner for this task.

## Outcome
Deliver a small backward-compatible Bash interface improvement and regression-tested PR, not an audit-only report or framework. Recurring production `label_too_long` (64-character cap declared in the schema) causes avoidable tool retries and token/roundtrip costs. Prefer making the valid path easy over asking the agent to rewrite optional presentation metadata.

## Bounded evidence and design
- Use existing production telemetry/logs for a representative bounded window across run/peek/wait/kill. Record exact host/version/window, counts and denominators. Separate input/schema rejection from command exit failure and runtime/network failure. Sanitize evidence; no secret payload dumps.
- Inspect declared maxLength64, prompt guidance, provider schema enforcement, backend validation and logging. Other parameter changes require observed data, not speculative cleanup.
- Choose smallest justified remedy: optional short-label guidance and/or deterministic server normalization only where presentation-only semantics permit. Do not blindly raise the cap. Preserve meaningful identity and audit trace; Unicode/boundary regressions required for normalization.
- Preserve WorkScope authority, operation exclusivity, handle identity and wait bounds. Never silently coerce commands/paths or hide actual command errors. Retain security/audit tracing.

## Acceptance
- Concrete code fix with focused boundary/provider/tool regressions and applicable spec updates; no new dashboard, persistent monitor or framework.
- Bounded before/after evidence: actual observed rates where available; otherwise distinguish representative corpus replay from production. Clearly label estimated avoided roundtrips/tokens and assumptions; no fabricated post-deploy measurements or guaranteed savings.
- Owning/full validation as appropriate, inspected diff, commit/push, qualified PR with hosted CI, exact-head review and explicit cursor-exhausted thread audit.
- Same implementation owner handles review fixes through completion. No merge/deploy authority; no production experiments, credential changes, broad cleanup or unrelated work.
