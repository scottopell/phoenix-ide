Reduce the measured multi-second latency of the no-query local branch picker without changing its API contract or touching durable-workflow concerns.

Production traces show GET /api/git/branches at p50 4.37s and max 9.96s across 10 returned samples, while REQ-PROJ-020 requires the local/no-network listing path to remain instant. The current implementation launches per-branch Git subprocesses to detect origin refs and behind counts, and runs worktree conflict discovery synchronously before the blocking task boundary.

## Scope

- Replace per-branch remote-existence subprocesses with one local remote-ref inventory.
- Preserve local branches sorted by recency, current/default branch data, behind-remote counts, and conflict slugs.
- Move all synchronous Git work off Tokio async workers.
- Add focused tests proving response parity for tracked, untracked, detached-HEAD, default-branch, behind-count, and worktree-conflict cases.
- Measure the endpoint before and after in a representative multi-branch repository and record raw samples.

## Constraints

- No network calls on the no-query path.
- No API/wire-shape changes.
- No branch/worktree lifecycle or durable-workflow changes.
- Do not weaken checked-out branch safety or conflict detection.
- If behind-count computation remains the dominant cost after batching remote-ref detection, document and spin it into a separate lazy-computation task rather than broadening this PR.
