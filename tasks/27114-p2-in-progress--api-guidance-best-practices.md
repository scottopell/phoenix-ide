Implement bounded, non-exhaustive Coordinator API guidance grounded in inspected current deployment/source contracts and explicit user authority. Preserve destination auth/TLS/credential, active WorkScope, immutable target/intent, endpoint-specific idempotency, fresh-read, and server enforcement boundaries. Add a verified structured-question response best-practice recipe without runtime changes or live API calls. Update prompt/built-in regressions, normative guidance, executive status and a short ADR. Qualify gap inventories by inspected deployment/source and permit verified supported alternatives and bounded state reads without inventing guarantees, receipts, private-handler/database bypasses, or background monitors. Permit exact user answers or decisions within explicit delegated decision scope, never generic inspect/unblock authority, fabricated approvals, or extended delegation. Validate with focused offline tests, formatting, diff and Allium checks; no local Rust builds, heavy checks, merge or deployment in this commission.


## Source delivery and qualification

Separate branch `fix/api-guidance-best-practices`, based on merged-main `46924e090fa72eb23c93a7594e53539682e30e6b`; PR 926 and PR 921 branches/evidence preserved. No runtime behavior changes.

- Six offline guidance/source drift tests pass. A first revised run failed one wording sentinel (`decision authority`); corrected guidance and reran all six successfully.
- `PHOENIX_CHECK_WORKERS=1 ./dev.py check --lanes cargo-fmt,task,spec-anchors` passes all three lanes. `git diff --check` passes.
- Built-in Allium check: zero errors, three existing warnings and informational trigger diagnostics. Source/wire/UI inspection and authoring pre-flight performed.
- Independent static review found two overconstraints: categorical gap inventory and human-only answer selection. Both corrected; independent final re-review reports no actionable finding. Review is bounded/static, not exhaustive proof.
- Rust prompt/skill/continuation regressions are authored; hosted CI remains required. No local heavy build or live Phoenix API/question call performed. No live instruction activation, merge, deployment or current-question answer authorized/performed.

User scope: “Submit new PR to relax relevant instructions, it is not my intent to prevent api usage not mentioned, only to establish best practices.” Global retains integration admission. Hosted exact-head results belong in the PR delivery record without source churn solely for CI bookkeeping.
