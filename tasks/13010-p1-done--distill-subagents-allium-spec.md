# Distill a subagents.allium spec (cross-domain seam — currently prose-only)

## Why this is a 1.0 release blocker

Sub-agent behavior sits at a cross-domain seam — projects' worktree ownership
↔ bedrock's sub-agent state space — which is exactly where bugs fall through
the cracks: each side assumes the other holds an invariant nobody actually
checks. Today the *precise* design lives only in `specs/subagents/design.md`
prose/tables (mode rules, model defaults, max-turns, the one-writer constraint,
the `cwd`-override scoping rule, the resume-path behavior) — `specs/subagents/`
has no `.allium`, and `specs/projects/projects.allium` §8 has only two thin
rules (`WorkSubAgentInheritsWorktree`, `ExploreSubAgentDirectory`) that don't
even model the `task.cwd` override. Concretely: "a Work sub-agent's overridden
`cwd` must stay inside the parent's worktree" is a `design.md` "should" with no
Allium invariant *and* no code guard (`handle_spawn_agents_tool` takes
`task.cwd` as-is). That's the anti-pattern AGENTS.md calls out — a safety
invariant living as prose. Sub-agents clear the "when to write Allium" bar
(real lifecycle: spawn → run → submit_result/submit_error/timeout → notify
parent → parent drains `AwaitingSubAgents` → re-Idle; plus a cross-spec
contract with bedrock and projects). Get this nailed down before 1.0.

## Scope

1. `/allium:distill` a new `specs/subagents/subagents.allium` from the code:
   `tools/subagent.rs`, `runtime/executor.rs::handle_spawn_agents_tool` (mode
   validation, one-writer checks, model/max-turns resolution), the
   `AwaitingSubAgents` drain in `apply_transition_result`, `runtime.rs` sub-agent
   spawn + per-mode tool-registry selection (`for_subagent_explore` /
   `for_subagent_work`, `with_mcp`), and the `SubAgentMode` / `SubAgentSpec` /
   `PendingSubAgent` / `SubAgentResult` / `SubAgentOutcome` types.

2. The spec should carry, with precise pre/postconditions and invariants:
   - **Mode rules**: `SubAgentMode::{Explore,Work}`; a Work sub-agent requires
     the parent in Work/Direct/Branch mode (Explore/none → rejected).
   - **One-writer invariant** — promote it from "referenced bedrock invariant"
     to an explicit, checked invariant: ≤1 Work sub-agent per `spawn_agents`
     call AND ≤1 Work sub-agent active at a time per parent (`active_work_subagents`),
     and the parent is in `AwaitingSubAgents` for the duration. Multiple Explore
     sub-agents in parallel are allowed.
   - **cwd-scoping invariant** (`WorkSubAgentWritesStayInWorktree` or similar):
     a Work sub-agent's *effective* cwd — including any `task.cwd` override — is
     inside the parent's worktree. An Explore sub-agent's cwd is the parent's
     worktree if it has one, else the parent's working_dir.
   - **Lifecycle**: spawn → run (bounded by `max_turns` and the timeout) →
     terminal via `submit_result` / `submit_error` / `timed_out` → parent
     notified → parent drains the sub-agent result buffer → re-enters Idle.
   - **Model & turn budget** as `@guidance` (these are config, not behavior):
     Explore defaults to the cheap model for the parent's provider, Work
     inherits the parent's model; an explicit `model` is validated against the
     LLM registry. Max-turns defaults: Explore 20, Work 50; over-limit forces a
     `submit_error("Reached maximum turn limit")`.
   - **Resume path** (`runtime.rs` ~1267): a restarted sub-agent gets the
     Explore registry + no MCP regardless of original mode — model it (or have
     the open-question resolution remove it; see below).

3. **Resolve open questions** (mandatory per AGENTS.md — each becomes a code fix
   or an explicit design decision via `AskUserQuestion`, not a prose note):
   - May a Work sub-agent's overridden `cwd` point *outside* the parent's
     worktree? Current code: yes (taken as-is). Almost certainly should be NO →
     add the guard in `handle_spawn_agents_tool` (reject with a clear error) so
     the cwd-scoping invariant actually holds.
   - Does an Explore sub-agent get the *full* MCP tool set or a search-restricted
     subset? Current code: full (`with_mcp(registry, mcp_manager)` for both
     modes). `design.md` marks the subset "deferred" — confirm "deferred" is
     still the answer (then the spec records it as a deferred refinement) or
     decide otherwise.
   - Is the resume-path "Explore registry + no MCP regardless of original mode"
     intentional (sub-agents don't survive restart, so it's effectively dead) or
     should it be removed/aligned?

4. **Code changes the spec implies** — at minimum the Work-sub-agent `cwd`-scoping
   guard; plus whatever else the open-question resolutions require.

5. **Reconcile the prose**: trim `specs/subagents/{requirements,design,executive}.md`
   so the Allium owns the detailed design (mode rules, model/turn tables,
   cwd-scoping, lifecycle) and the `.md` files keep only user-need + rationale +
   status, per AGENTS.md ("when an Allium spec exists, the spec is the
   authoritative source for ... invariants and operation sequences"). Decide
   whether `projects.allium` §8's `WorkSubAgentInheritsWorktree` /
   `ExploreSubAgentDirectory` get folded into `subagents.allium` (with
   `projects.allium` `use`-ing it for the cross-spec contract) or stay as thin
   references. Update `specs/projects/executive.md` REQ-PROJ-008: it's
   essentially complete (mode param, model override, one-writer, MCP access,
   max-turns, cwd inheritance all implemented — the current "🔄 Partial" note is
   stale on all four counts); the only real remainder is the cwd-guard + the
   open-question resolutions. Also fix the stale `design.md` "Working Directory
   Assignment" table cell that still says the Explore parent's cwd is the "main
   checkout" (it's the Explore worktree post-REQ-PROJ-028).

## Out of scope

- Changing the sub-agent feature itself — mode/model/max-turns/one-writer are
  implemented and fine; this is about pinning the spec down + closing the
  cwd-guard hole, not redesigning.
- Implementing the Explore-sub-agent search-restricted MCP subset — just decide
  whether it stays deferred; if it shouldn't, that's a separate task.

## Acceptance

- `specs/subagents/subagents.allium` exists, distilled from the code, with the
  invariants above; `allium check specs/subagents/subagents.allium` clean (no
  errors, no findings, no open questions left as prose).
- The Work-sub-agent overridden-`cwd`-must-be-inside-the-worktree rule is a
  checked Allium invariant AND enforced in `handle_spawn_agents_tool`; a test
  covers the rejection.
- `specs/subagents/{requirements,design,executive}.md` trimmed to user-need +
  rationale + status; no detailed design duplicated between the `.md` files and
  the `.allium`.
- `projects.allium` §8 either folds into / `use`s `subagents.allium` or keeps a
  thin documented cross-spec reference; `executive.md` REQ-PROJ-008 status
  updated; the stale design.md "main checkout" table cell fixed.
- `./dev.py audit-specs` clean; `./dev.py check` green.
