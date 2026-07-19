# Sanitized guidance evaluation

These scenarios paraphrase initial requests from the Phoenix production corpus. Incidental names, paths, IDs, quotes, and implementation answers are removed. They test investigation routing, not whether an agent memorized the historical fix.

## Scoring

For each scenario, score one point for each expected obligation identified **before implementation**. Subtract one for each irrelevant deep branch that could not change the plan. A hard failure is proposing a fix without tracing the named artifact or without an acceptance path.

Compare:

- **Legacy development guidance:** server lifecycle, generic tests, module layout, old tool paths, and `phoenix-client.py`.
- **Corpus-derived guidance:** the current `phoenix-development` + `phoenix-explore` loop and only the selected reference sections.

Evaluation may use an agent rubric or manual review. It must not query the original conversation while scoring.

## Calibration scenarios

These informed the skill draft.

### C1 — Pending message remains after restart

**Request:** A message accepted before a process restart remains pending after the app returns. Find the root cause and prepare a task.

**Expected obligations:**

- distinguish UI optimistic/pending state from persisted authoritative state;
- trace restart recovery/runtime materialization and state-machine acceptance;
- inspect SQLite rows/migrations and idempotent wake/reconciliation;
- include SSE Init/replay/reducer reconstruction;
- test crash/restart plus duplicate/stale completion.

### C2 — New-page skill autocomplete is inconsistent

**Request:** Skill suggestions on the new-conversation page sometimes omit a project-local skill or appear after the user has already typed.

**Expected obligations:**

- exact discovery/API/request path and new-page owning state;
- worktree/project cwd and discovery precedence/dedup;
- async race/stale response cancellation;
- backend discovery tests plus focused React interaction test;
- browser journey for typing/selection timing.

### C3 — Production self-deploy can strand the app

**Request:** A production self-deploy occasionally leaves no usable Phoenix process. Design a safe fix.

**Expected obligations:**

- deployed evidence first: bounded traces/logs/process/version;
- launchd/systemd/daemon ownership distinction and deployment skill/spec;
- handoff/rollback crash points and transactional ownership;
- disposable end-to-end lifecycle harness, not live experimentation;
- clear operational non-goals and recovery behavior.

### C4 — Tool timer disappears too early

**Request:** The elapsed timer on a tool card disappears when output arrives, even though work is still in progress; the symptom is not limited to one tool.

**Expected obligations:**

- treat the visible card as a shared rendering/status concern, not tool-specific logic;
- identify authoritative phase/completion timestamp;
- inspect tool result vs conversation state and SSE/reducer derivation;
- check cancel/retry/stale outcomes;
- component test plus browser timing journey without magic sleeps.

### C5 — Cmd/Ctrl+F misses off-screen viewer matches

**Request:** Find works in one viewer but misses matches outside the rendered window in another.

**Expected obligations:**

- logical content/search projection vs mounted DOM/virtualization;
- focus-scope and topmost viewer ownership;
- shared viewer-find primitive vs parallel implementations;
- navigation/stable match handles and overlay behavior;
- focused tests plus real keyboard/browser journey at multiple viewports.

### C6 — Provider drops tool-result images

**Request:** One provider can receive text tool results but silently loses images. Plan the correct integration fix.

**Expected obligations:**

- trace typed content from tool executor through provider adapter;
- distinguish transcript/UI metadata from model-visible content;
- avoid duplicate byte representations;
- make capability gap structural where possible and log unavoidable drops;
- provider-focused tests and spec impact.

## Holdout scenarios

These were reserved until after the first draft.

### H1 — Conversation list jitters after status updates

**Request:** Rows in the conversation list move or resize when background status changes, especially on mobile.

**Expected obligations:**

- identify stable row identity/layout and status producer;
- distinguish list virtualization/measurement from data reordering;
- trace background state/SSE updates into derived selectors;
- inspect colocated/responsive CSS and shared adornment slots;
- fixture/browser geometry evidence plus regression test.

### H2 — Continued conversation uses a stale worktree

**Request:** Continuing completed work sometimes opens against a worktree that should have been reclaimed.

**Expected obligations:**

- conversation cwd, continuation chain, work-scope/task lifecycle;
- all-worktree/checked-out-ref ownership before cleanup/ref movement;
- persisted terminal/reclaim state and restart behavior;
- remote PR/branch observation vs local ownership;
- idempotent cleanup/conflict fallback tests.

### H3 — Retriable error advice seems wrong

**Request:** The UI recommends retrying a specific provider error, but the user doubts that the operation is actually safe to retry.

**Expected obligations:**

- trace the exact error string/classification before answering;
- provider adapter → common error taxonomy → retry/backoff → UI copy;
- side-effect/idempotence and cancellation during backoff;
- parent vs sub-agent visibility;
- exact classification regression and user-visible status test.

### H4 — Reveal-in-folder appears for a remote browser

**Request:** A browser on another machine sees an action to reveal a server path in Finder/Explorer.

**Expected obligations:**

- server filesystem vs browser-host boundary;
- server-side same-host peer detection and trusted loopback-forwarded-header gate;
- `DeploymentInfo.local_access` producer/consumer plus endpoint recheck;
- containing-folder-only behavior;
- direct-remote, loopback proxy, forged-header, and UI visibility tests.

## Evaluation results

Calibration scenarios received manual rubric review. Holdout scenarios were also sent blind to independent agents in paired runs: one received only the legacy skill summary; one read the new Explore guidance and selected reference sections. The table records obligations visible in their returned plans.

| Scenario | Legacy obligations found | New obligations found | Irrelevant deep branches | Result |
|---|---:|---:|---:|---|
| C1 | 1/5 | 5/5 | 0 | improved |
| C2 | 2/5 | 5/5 | 0 | improved |
| C3 | 1/5 | 5/5 | 0 | improved |
| C4 | 1/5 | 5/5 | 0 | improved |
| C5 | 1/5 | 5/5 | 0 | improved |
| C6 | 1/5 | 5/5 | 0 | improved |
| H1 | 3/5 | 5/5 | 0 | holdout pass |
| H2 | 3/5 | 5/5 | 0 | holdout pass |
| H3 | 3/5 | 4/5 | 0 | holdout partial |
| H4 | 2/5 | 4/5 | 0 | holdout partial |

The unguided/legacy agents still supplied useful general engineering knowledge, so the comparison is not “old finds nothing.” The new guidance consistently supplied Phoenix-specific authority and seam routing. It also made the plans explicitly conditional, avoiding unrelated branches.

### Evaluation-driven revisions

- The first draft over-emphasized mandatory multi-surface breadth for contained UI changes. The final wording makes branches conditional: inspect the exact artifact and one layer on each side of the likely seam, and skip any branch that cannot alter the plan.
- Holdout H3 omitted operation idempotence/side effects and parent-vs-sub-agent retry scope. The provider cue now names both.
- Holdout H4 omitted endpoint re-authorization, proxy/forged-header cases, and containing-folder-only behavior. The locality cue now names endpoint recheck, proxy behavior, and the folder-only constraint.

These revisions preserve breadth for cross-boundary failures without forcing architectural excavation for local styling work. H3/H4 are reported as partial rather than retroactively rescored; a future refresh should use new unseen holdouts to evaluate the revised cues.
