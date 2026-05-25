# Browser Sessions on WorkScope

## Goal

Migrate `BrowserSessionManager` from per-conversation ownership to `WorkScope`-keyed ownership, integrate browser session teardown into the unified resource cleanup cascade, and tighten the cascade signatures across all three resources (tmux + projects + browser) to take `&WorkScope` consistently. Result: a Playwright session created in a worktree-backed conversation is the same Chrome window a context-exhaustion continuation inherits, and archive/abandon/mark-merged/delete kill the Chrome process the same way they kill bash and tmux today.

## Why

Two related production pains, both flagged by the persona panel during PR #133 / #135 / #136 triage:

1. **Cleanup leak.** Browser sessions are scoped per-conversation but never appear in the resource-cleanup cascade. Archive/abandon/mark-merged today kill bash + tmux + worktree (PR #135) but leak the Chrome process. Sessions accumulate until Phoenix restart.
2. **Silent cross-continuation breakage.** When a Work/Branch conversation continues into a new conversation row (context exhaustion, etc.), the continuation finds no session and silently opens a fresh one. Auth state, cookies, and open tabs from the predecessor are gone. Symptom looks like "the agent re-logged in for no reason." Itzel called this out as the most expensive silent failure mode in the current ownership model — louder than bash but quieter than tmux.

## Foundation already in place

- **PR #136** (`feat: introduce WorkScope; key tmux server by work scope`): `WorkScope` enum, `ToolContext::work_scope`, tmux server registry keyed by `stable_key()`. This task uses the same primitive for browser sessions.
- **PR #135** (`fix: unify lifecycle resource cleanup; chain-aware worktree preservation`, stacked on #136): `run_resource_cleanup_cascade` factored out so archive/abandon/mark-merged/delete share one teardown path. `cascade_projects_on_delete` already skips non-leaf chain members — the same leaf-only insight applies here.

This task is phase 1.3 of the foundation work; phase 2 (long-running tasks / watch shape) is gated on this landing.

## Design decisions (locked)

Each was resolved via `/asking-questions` after the persona-panel synthesis. Recorded here so the implementer doesn't relitigate.

- **Inherit-session granularity = same Chrome window.** Keep the existing `Arc<RwLock<BrowserSession>>` alive across continuation. The continuation sees the predecessor's open tabs, current URL, scroll position, dev-tools state — not just cookies. Cheapest implementation (the session is already an `Arc`, just don't drop it on continuation) and most useful to agents. Trade-off accepted: if a tab was mid-render or had a modal up at continuation time, the new agent inherits that state.
- **Cascade signature scope = tighten all three.** This PR refactors `cascade_tmux_on_delete` and `cascade_projects_on_delete` to take `(&WorkScope, continued_in_conv_id)`, and adds `cascade_browser_on_delete` with the same signature. Eliminates the inline `tmux_worktree_buf = conv.conv_mode.worktree_path().map(...)` derivation at cleanup callsites and gives all three cascades a uniform shape. Larger PR, cleanest end state. Accept conflict surface against PR #135's just-shipped code.
- **Spec coverage = REQs + new `browser-lifecycle.allium`.** Markdown REQs in `specs/browser-tool/requirements.md` for ownership + cascade integration, plus a new Allium spec describing session states (`Uninitialized | Live | InheritedByContinuation | TornDown`) with preconditions on each transition. Cross-continuation inheritance is a lifecycle transition with preconditions — matches AGENTS.md guidance for when Allium is warranted. Catches ambiguities (cleanup racing continuation creation) before they ship as bugs.
- **Pre-existing in-memory sessions = no runtime rekey.** Sessions live only in process memory. Rollout is "restart Phoenix"; old per-conversation entries die with the old binary. No backward-compat shim needed; document the rollout note in the PR body.

## Scope

1. **Rekey `BrowserSessionManager` off `WorkScope`.**
   - Internal map keyed by `WorkScope::stable_key()` (disjoint `worktree:` / `conversation:` namespaces).
   - Worktree-backed conversations and their continuations resolve to the same scope key → same `Arc<RwLock<BrowserSession>>` → same Chrome window.
   - Direct conversations resolve to `WorkScope::Conversation(id)` → sessions remain per-conversation (no behavior change; Direct continuations get a fresh session, which is correct because they have no durable owner).
   - Public surface: `get_session(&WorkScope)` becomes the primary call. `ToolContext::browser()` resolves `&self.work_scope` internally so tool callsites don't change.

2. **Tighten cascade signatures + integrate browser teardown.**
   - `cascade_tmux_on_delete(&WorkScope, continued_in_conv_id)` — replaces today's `(conv_id, worktree_path, continued_in_conv_id)`.
   - `cascade_projects_on_delete(&WorkScope, continued_in_conv_id)` — same.
   - New `cascade_browser_on_delete(&WorkScope, continued_in_conv_id)` — mirrors the others. Skips teardown when `continued_in_conv_id.is_some()` (worktree-scoped sessions belong to the leaf, same chain-preservation pattern as `cascade_projects_on_delete` in PR #135).
   - Cleanup callsite in `handlers.rs::run_resource_cleanup_cascade` constructs `WorkScope` from the conversation once, passes `&WorkScope` to all three cascades.
   - Failures log WARN and continue; consistent with bash/tmux cascade error policy.

3. **Capability-gap logging.**
   - When a `BrowserSessionManager` lookup finds no existing session for its scope (including continuation-time "expected to inherit but didn't"), log at `debug` so leak / cross-continuation regressions are not silent. Per panel feedback (Yusuf): the absence of an event is the failure mode that hides hardest.

4. **Spec coverage.**
   - Extend `specs/browser-tool/requirements.md` with `REQ-BROWSER-WS-001..NNN` for: WorkScope ownership, continuation inheritance, cascade integration. Canonical `### REQ-...` headings so `./dev.py check`'s spec-anchors lane stays green for code anchors.
   - Update `specs/browser-tool/executive.md` status table.
   - Add `specs/browser-tool/browser-lifecycle.allium`: session states, transitions, preconditions, invariants. Run `/allium:propagate` after design.md sketch to generate tests; resolve any open questions via `AskUserQuestion` before merging (mandatory per AGENTS.md).

## Non-goals

- Do not introduce browser-session DB persistence. Sessions remain in-memory and runtime-scoped.
- Do not change the user-facing browser tools' input schemas (`browser_navigate`, `browser_click`, etc.).
- Do not address watch / long-running-task shape (phase 2). This task is the last piece of phase 1 foundation.
- Do not migrate ordinary bash handles to WorkScope ownership (explicit non-goal from the #133 / #136 spec discussion).
- Do not add a `browser_handoff` tool or any explicit cross-WorkScope transfer API. Continuation inheritance is automatic and structural; agents do not opt in.
- Do not write a backward-compat shim for pre-existing per-conversation session entries. Rollout is "restart Phoenix."

## Validation

- `./dev.py check` — all 17 lanes pass.
- Existing browser-tool tests pass with updated `get_session` signature.
- New tests: continuation inheritance (Work/Branch); per-conversation isolation (Direct); leaf-only cascade preservation (chain non-leaf does not kill Chrome); cleanup-after-leaf fires teardown exactly once.
- Allium-generated tests for the lifecycle state machine.
- Manual smoke: open a browser session in a Work-mode conversation, force a context-exhaustion continuation, verify the continuation sees the same tabs and URL.
