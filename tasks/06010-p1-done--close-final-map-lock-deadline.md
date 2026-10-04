# Bound final tmux retirement map-lock deadline deterministically

Replace the scheduler-sensitive close-deadline regression with deterministic lock-readiness and clock control. Preserve the finite Close deadline, failure outcome visibility, and retained registry membership. Validate focused test/module, full checks, hosted CI, and review; do not merge or deploy.


## Completion receipts

- PR: https://github.com/scottopell/phoenix-ide/pull/816
- Source head: `74c01a4bf0675f78825d1dbcb89ea28c6f61a0ca`
- Prior failure: PR #814 Actions run 36589307710, job 109478166471 stopped at 514 passed / 1 failed / 1,667 unrun; the assertion did not print the divergent `TmuxRetirementOutcome`.
- Cause: the 100ms permit deadline started before entry-lock, authority-validation, and map-lock orchestration, so scheduler delay could exhaust it before the intended final-map phase.
- Fix proof: paused Tokio time, explicit final-authority readiness, held registry map lock, explicit final-map-lock attempt readiness, then virtual deadline advance. The original finite Close deadline remains authoritative; the expected typed residual and retained membership are asserted, and divergent outcomes are printed.
- Focused validation: exact regression 1/1 passed; registry module 52/52 passed; phoenix-tools all-target clippy and Rust timing lint passed.
- Hosted CI run 36593469643 passed planning, Rust, clippy, e2e, UI/specs, and task validation; hosted Rust passed all 5 checks in 463.2s.
- Exact source-head Codex review found no major issues; zero unresolved threads.
- Local `./dev.py check --all` was not green: after compilation consumed available disk, 7/19 checks failed with explicit ENOSPC errors. This environmental receipt is preserved and was not retried.
- No production behavior changed. No merge or deployment was performed.
