# Diagnose recurring tmux test watchdog cleanup failures

Recurring post-adoption watchdog exit-1 failures block production deployment even though isolated re-runs pass. Preserve task 57005 and PR #781 history; this follow-up must distinguish terminal cleanup branches with bounded non-secret receipts, reproduce the causal fixture race, and make the narrowest test-support-only repair while retaining exact PID + kernel birth + immutable token + socket inode containment, replacement protection, and fail-closed cleanup.

Acceptance: deterministic regression where feasible; repeated focused tests at representative concurrency; owning watchdog and rehydration suites; exact-HEAD `./dev.py check`; commit/push one PR and qualify hosted CI plus exact-head Codex/cursor review. No production tmux behavior changes, broad process scanning/signaling, timeout inflation, swallowed errors, deploy, or merge.
