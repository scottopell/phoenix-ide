Restore the established asynchronous ProductConversation creation contract after the directory-first cutover. A structurally valid request must durably accept promptly, then the durable creation worker—not the HTTP request—must own repository pinning, detached-worktree materialization, atomic publication, retries, and recovery. The UI must observe the exact request through publication/failure and navigate only after atomic publication. Ensure accepted and expired rows recover without another request or restart, cancellation fences late effects, external Git/lock operations are bounded/cancellable, and durable stage diagnostics identify failures without leaking credentials. Preserve exact detached pins and prohibit creation-time commits, tags, and local branches. Required regressions cover blocked Git acceptance latency, dropped HTTP waiters, lost wakes/expired leases, cancellation fences, and exact-request UI observation. Qualify with focused tests, full checks under host admission, adversarial exact-HEAD review, commit/push, and PR. Do not deploy or replay the cancelled Datadog request.


## Completion evidence

- PR: #916 (`fix/async-product-creation-worker`)
- Qualified implementation head: `186e76cec724f5b8fb56bd86d9b131a6b16ea179`
- Linux CI: run `38703306394` — Rust, cargo-fmt, clippy, UI/specs, task validation, E2E, and musl production-feature checks passed.
- macOS helper qualification: run `38703306367` passed.
- Final immutable range review: `d69cde67597605dffdb5739647220820cc8f3aa8..186e76cec724f5b8fb56bd86d9b131a6b16ea179` — no actionable findings.
- Deployment and merge were not performed or authorized.
