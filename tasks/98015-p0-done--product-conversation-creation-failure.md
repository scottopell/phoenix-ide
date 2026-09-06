# Fix ProductConversation creation failure at its evidenced boundary

Investigate the reported failure to create a ProductConversation rooted at `/Users/scottopell/dev/phoenix-ide` while preserving the verified control: a dirty canonical checkout must not prevent isolated managed-worktree creation.

Before editing, obtain at least three fresh independent read-only reviews covering (1) `NewConversationPage`/`useCreateConversation` request identity and recoverable error presentation, (2) POST acceptance/idempotency through creation claims, Git authority, and worktree provisioning, and (3) production traces plus recursive transcript-query performance on canonical route load. Query production evidence narrowly around recent creation attempts and reproduce non-destructive failure variants. Synthesize and try to falsify each proposed cause.

Implement only an evidenced root cause. If the original failure cannot be recovered, make the smallest structural observability/recoverability change that exposes exact rejection or provisioning errors without masking them. Do not add dirty-checkout gates or workarounds, speculative retries, timers, or state machines.

Acceptance requires end-to-end regression coverage proving a dirty canonical checkout does not block isolated worktree creation; focused tests and `./dev.py check`; adversarial review of the dirty diff; task completion; commit/push; and opening or updating a PR. Report concrete evidence and whether the 1–3.6 second recursive transcript query is causal or separate. Do not deploy or touch #745/#702.
