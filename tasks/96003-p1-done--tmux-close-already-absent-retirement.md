# Fix exact tmux disappearance during ProductConversation Close retirement

## Context and concrete reproduction evidence

Commissioned real-browser QA on exact merged main `d1f0e3f9f6cdd683e4c3d84e96b0d05fe8b6d1ee` found an intermittent ProductConversation Close regression while validating rebased PR #702:

- one complete isolated six-journey run passed, including clean Close to durable History;
- the next strengthened full run failed at `POST /api/conversations/<id>/close/retry-retirement` with HTTP 409 `close_retirement_needs_repair`;
- the exact detail was `tmux socket incarnation was unavailable before exact teardown`;
- QA preserved uncommitted oracle improvements and stopped without adding backend logic.

The preserved PR #702 worktree changes only QA files. Its strengthened clean-Close oracle proves that the attached worktree exists before Close and is absent afterward, the stable ProductConversation reaches History with no active Close state, the transcript remains present, and the browser surface is read-only. The preserved `phoenix.log` no longer contains the reported request context, so the HTTP/error evidence is the commissioned QA report; the emitting path and strengthened oracle are locally verified.

## Failure model

`TmuxRegistry::begin_retirement_inner` seals a `TmuxRetirementPermit` containing the exact `WorkScope`, socket path, Phoenix-controlled server token, and retirement generation. `TmuxRegistry::complete_retirement` then re-finds the entry, locks it, and verifies exact pointer/generation/path/token ownership before sampling the socket inode/device for token-bound teardown.

There is a real asynchronous boundary after that exact-authority check and before `socket_file_identity`. The tmux server/OS owns server exit and socket unlinking; the exact server can disappear in that window. On merged main, `socket_file_identity(...) == None` is classified as `IdentityNotProven("tmux socket incarnation was unavailable before exact teardown")`. The Close runtime maps that result to residual evidence, moves the exact attempt to `NeedsRepair`, and retry returns the observed 409 when retirement cannot converge.

This explains pass-then-fail without cross-run state leakage: each isolated run independently races exact tmux exit/socket unlink against pre-teardown inode capture. A missing socket after exact permit authority was established is an already-absent exact resource, not lost authority. By contrast, an existing dead socket, unreadable or changed token, changed registry entry/generation, lock/deadline failure, or other ambiguous observation remains lost authority and must fail closed.

Relevant anchors on exact main:

- `crates/phoenix-tools/src/tmux/registry.rs`: `TmuxRetirementPermit`, `build_retirement_permit`, `matches_exact_instance`, `complete_retirement`, `verify_exact_absence`, and final registry-authority removal.
- `crates/phoenix-tools/src/work_scope_inventory.rs`: read-only tmux inventory projection.
- `crates/phoenix-ide/src/runtime/close_retirement.rs`: lease acquisition/inventory, `complete_close_resource_lease`, `tmux_retirement_outcome`, durable rehydration, evidence recording, and completion.
- `crates/phoenix-ide/src/api/lifecycle_handlers.rs`: `retry_close_retirement` exact-attempt API behavior.
- `crates/phoenix-db/src/close_foundation.rs`: retry, dispatch/evidence persistence, `NeedsRepair`, and idempotent completion.
- `specs/work-lifecycle/requirements.md`: REQ-WL-002b, REQ-WL-002d, REQ-WL-002c.
- `specs/work-lifecycle/work-lifecycle.allium`: exact-resource retirement, already-absent evidence, replacement preservation, retry, and completion rules.
- `specs/adrs/039_durable-runtime-resource-identity-fails-closed.md`: durable exact-instance authority and replacement safety.

Git history identifies PR #731 commit `daa0b6d8740b` as the main exact-incarnation retirement rewrite, followed by exact rehydration and deadline/lock bounding commits before PR #738 merged it into the aggregate-authoritative Close flow.

## Boundary

Implement the smallest typed tmux retirement transition that distinguishes:

1. exact permit authority followed by an absent socket before teardown — consume as exact already-absent evidence without issuing a kill;
2. proven absence of the sealed server with a replacement at the reused socket — record absence and leave the replacement untouched;
3. missing, changed, or ambiguous authority — preserve the resource and remain in typed `NeedsRepair`.

Keep final registry consumption conditional on the same exact entry and permit. Do not reinterpret an existing but unprovable socket as absence, and do not turn path absence alone into general-purpose retirement authority outside the exact live permit transition.

Task 92033 remains the broad historical owner of Close settlement and WorkScope retirement orchestration. This focused post-merge regression task owns only the tmux already-absent transition, its propagation through existing Close retry/recovery semantics, and regression proof. It must not reopen or duplicate the rest of task 92033.

## Implementation plan

1. Add a deterministic test-only synchronization seam immediately after `complete_retirement` proves exact registry/permit authority and before it samples socket incarnation. Use notification/barrier coordination consistent with existing registry race tests; do not use polling, sleeps, or timing luck.
2. Add an old-main regression that creates a real exact tmux server and permit, pauses at that seam, removes the exact server so its socket is absent, then releases completion. Assert the fixed typed outcome is exact absence/retirement rather than `IdentityNotProven`; assert the exact registry fence/entry is consumed correctly.
3. Refine the production transition with an explicit typed observation/result rather than a string special case. Exact-authority-plus-observed-absence must bypass destructive teardown and continue through final exact registry-authority validation. Preserve every fail-closed path for dead sockets, unreadable/changed tokens, changed entries/generations, and deadlines.
4. Add or extend a focused matrix proving:
   - an already-absent exact resource converges idempotently;
   - truly lost/ambiguous authority remains `IdentityNotProven`/`NeedsRepair`;
   - a replacement incarnation at the reused socket is never killed or removed from the registry;
   - repeated completion/retry cannot target a replacement and preserves exact-attempt evidence;
   - restart rehydration of an absent sealed server records exact-attempt absence, while ambiguous restart identity remains repair-class.
5. Add a focused Close-runtime/API-orchestration regression where useful to prove the tmux typed outcome is persisted as absence evidence and the same exact attempt can converge through retry/restart/idempotent replay to History. Reuse existing Close fixtures and typed persistence APIs; do not fabricate a new Close path or duplicate backend behavior in PR #702.
6. Update only verification/status documentation if necessary. The normative requirements and Allium rules already require exact absence, fail-closed ambiguity, replacement preservation, and same-attempt retry; do not broaden compatibility policy or rewrite those contracts merely to restate the implementation.

## Acceptance

- The deterministic pre-inode-capture regression fails on `d1f0e3f9` with the commissioned QA error and passes after the fix without sleeps or polling.
- Exact permit authority followed by socket absence produces typed exact-attempt absence/retirement evidence and can complete Close to durable History.
- Path absence without the exact permit/retained durable identity does not confer authority.
- Dead socket, unreadable token, changed identity, missing permit, and deadline/lock failures remain fail-closed and visible as typed repair.
- A replacement/user-owned tmux server at the same socket path is never signaled, killed, unlinked, or removed from registry authority.
- Same-attempt retry, repeated completion, and restart rehydration are idempotent and preserve prior evidence rather than minting a new attempt or ignoring missing evidence.
- Existing tmux replacement, rehydration, deadline, admission-fence, Close persistence, and History tests remain green.
- No polling or sleeps are introduced as correctness or test synchronization.

## Validation and delivery

- Run focused `phoenix-tools` tmux retirement tests and focused Close runtime/DB/API tests.
- Run applicable `./dev.py check` lanes, including formatting, clippy, tests, spec checks, and task validation.
- Obtain multiple fresh independent adversarial reviews of the exact candidate head and resolve every finding.
- Mark this task done in its own clean commit, push the owned branch, and open/update a focused PR.
- Request exactly one Codex review at the final exact head, wait for CI at that head, and paginate all PR review comments/threads to require zero unresolved actionable threads.
- Never merge, deploy, start UAT, or modify QA PR #702 or iOS PR #745.

## Explicit non-goals

- Weakening socket-path-plus-token incarnation matching or killing by socket path alone.
- Making Close succeed by discarding missing evidence or suppressing `NeedsRepair`.
- Polling/sleeps as synchronization or correctness.
- A broad tmux framework rewrite, global retirement coordinator, compatibility/downgrade behavior, or unrelated lifecycle changes.
- Changes to PR #702 QA or PR #745 iOS code.
- Merge, deployment, or UAT.
