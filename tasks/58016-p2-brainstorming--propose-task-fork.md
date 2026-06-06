# Work-mode task fork: propose_task that spawns a decoupled conversation

## Summary

Extend `propose_task` so an agent in **Work** mode can hand off a piece of
newly-discovered, self-contained work to a brand-new, independent conversation —
without halting its own work and without any lifecycle coupling to the spawned
conversation.

Motivating example: a conversation refactoring database connections discovers a
bug in TLS termination. Rather than carrying that context (and the mental load),
it proposes the TLS fix as a separate task. The originating conversation
continues uninterrupted; if the user approves the proposal, a fresh top-level
conversation is created to do the TLS work, fully decoupled — a fork, but with
no parent/child lifecycle relationship of any kind.

## Framing: same hand-off pattern, one new axis

`propose_task` already expresses "an agent decides to hand a unit of work it
understands to a separate conversation." Its approval today offers two outcomes,
which are really two cells of a 2x2 over (where the task runs) x (what happens to
the originator):

|                              | Originator continues | Originator stops                                   |
| ---------------------------- | -------------------- | -------------------------------------------------- |
| Task runs in THIS conversation | n/a                | ContinueInCurrentConversation (Explore -> Work)    |
| Task runs in a FRESH conversation | THIS FORK (new)  | StartFreshWorkConversation (parent + chain links; predecessor -> HandedOff) |

The fork is the missing cell: **fresh conversation + originator keeps going,
untouched.** The only new axis is "does the originating conversation keep going?"
Everything else (writing a task file, the full-screen approval UI, spawning a
fresh Work conversation off base) is reused.

Therefore this is NOT a new tool. Make `propose_task` available in Work mode and
add a "fork" approval outcome.

## Key behaviors (decided)

1. **Non-blocking proposal.** In Work mode, calling `propose_task` does NOT park
   the conversation (contrast Explore's `AwaitingTaskApproval`). The proposal is
   recorded and surfaced in the tool output; the conversation immediately
   continues its own work. Explore-mode behavior is unchanged.

2. **Asynchronous, human-gated approval.** The user reviews the proposal in the
   tool output whenever they choose, and opens the *same* full-screen
   task-framing/approval interface used today. Approval is what creates the fork.
   The decision arrives asynchronously; the originating conversation never waits
   on it.

3. **Spawned conversation = fresh top-level Work conversation, new chain.**
   Like `StartFreshWorkConversation` but cut from the **repository default branch**
   (the project's `main_ref`), NOT the origin's `base_branch` (which for a
   Branch-mode origin is the PR branch it is editing). Its own worktree, its own
   task branch, the approved task file committed. Appears as an independent root in
   the conversation list (its own chain).

4. **No lifecycle relationship.** Model the spawn on the Chain Q&A fire-and-forget
   pattern (`ChainQa::submit_question`): no `parent_event_tx`, no
   `AwaitingSubAgents`, no `SubAgentResult`, no `continued_in_conv_id`. The
   originator receives ZERO notifications of the fork's progress, completion, or
   failure, and does NOT transition to `HandedOff`.

5. **Non-notifying provenance breadcrumb.** Add a dedicated field, e.g.
   `spawned_from_conversation_id`, DISTINCT from `parent_conversation_id` (which
   carries sub-agent/handoff lifecycle meaning) and from `continued_in_conv_id`
   (chains). Provenance only — a UI/audit breadcrumb with zero notification
   semantics.

## Design work to resolve during implementation

- **Pending-proposal persistence.** Because approval is async and the originating
  conversation moves on, the pending fork proposal must be persisted (keyed to
  originating conversation + tool-call id) and surfaced so the user can act later.
  Decide the storage (new table/row vs. column) and the API entry point for
  "approve this proposal -> spawn fork", analogous to the task-approval endpoint
  but NOT tied to a parked `AwaitingTaskApproval` state.

- **Interception branch.** `propose_task` is intercepted in the state machine
  (`transition.rs` `handle_core_llm_response`, `resolve_task_file`). Add a
  mode-aware path: fork-eligible modes (Work / Branch / Direct-in-git) -> record
  proposal + return to `LlmRequesting` (continue); Explore mode -> existing
  `AwaitingTaskApproval` (unchanged).

- **Proposal-specific approve path (NOT `TaskApprovalOutcome`).** A fork origin never
  enters `AwaitingTaskApproval`, so fork approval must NOT be a new `TaskApprovalOutcome`
  value (that routes through the parked Explore approval and would reject — origin isn't
  awaiting — or mutate the origin). Instead add a dedicated async endpoint
  (`/proposals/:id/approve`) dispatching `Effect::SpawnFork`, with an executor handler
  analogous to `execute_approve_task_fresh_handoff` but: keyed on the proposal id, no
  chain/parent links, sets the `spawned_from` breadcrumb, and does NOT touch the
  originating conversation's state.

## Mode availability (decided)

`propose_task` currently lives only in the `explore_*` registries
(`crates/phoenix-tools/src/lib.rs`). The fork form is available from:

- **Work** — yes.
- **Branch** — yes.
- **Direct** — yes, but ONLY when the working directory is a git repository.
  Direct-not-in-a-repo has no HEAD to branch from, so the tool is not offered.
- **Explore** — unchanged (existing parking/handoff behavior; not the fork form).

Direct origins are interesting because the *origin* has no worktree/branch
ceremony, yet the *fork* is a managed-style conversation (it gets its own
worktree/branch). The fork is managed even when its origin was not.

## Worktree / branch / base (decided)

Uniform across all fork-eligible origin modes:

- **Fresh worktree** for the spawned conversation (never shares the originator's
  worktree — that would break the one-writer invariant and entangle two unrelated
  changes).
- **New branch cut from the repository's default branch, NOT from the originator's
  `base_branch` or `HEAD`** — uniformly for every origin mode (Work, Branch,
  Direct-in-git). The default branch is the project's mandatory, immutable
  `main_ref` (resolved at project creation: remote default when detectable, else
  the checked-out branch). The fork is an independent unit of work: it diffs only
  its own changes and is reviewable / mergeable on its own, with no entanglement
  with the originator's in-progress work.
  - A Branch-mode origin's `base_branch` equals the branch it is editing, so it is
    explicitly NOT used as the fork base — that would stack the fork on the
    origin's unmerged PR branch.
- Consequence (accepted): the fork does NOT inherit the originator's in-progress
  state. The discovered work must be self-contained enough to stand on the default
  branch; if it genuinely depended on the originator's uncommitted changes, this
  hand-off would be the wrong tool.
- Reuse the managed-conversation creation path (worktree, branch, task-file
  commit) and respect "Git worktrees are owned environments" — never move the
  originator's branch ref.

## Specs to update

- `specs/projects/projects.allium` — new approval-outcome rule (e.g.
  `TaskApprovalForkExecuted`): fresh successor, no parent/chain links, originator
  unchanged; `propose_task` availability in Work mode.
- `specs/projects/design.md` — data model for `spawned_from_conversation_id`; the
  fork flow and its non-coupling guarantees.
- `specs/bedrock/design.md` — Work-mode `propose_task` interception is
  non-parking (async proposal), distinct from Explore's `AwaitingTaskApproval`.
- DB migration for the provenance field and any pending-proposal storage (not
  `serde(default)` alone — see AGENTS.md schema-evolution rule).

## Correctness guardrails (AGENTS.md)

- Do not overload `parent_conversation_id` / `continued_in_conv_id`; the
  breadcrumb is its own field. No parallel representations of the same value; no
  structural ambiguity between "deliberately decoupled" and "forgot to thread".
- The "no lifecycle notification" guarantee should be correct-by-construction:
  the fork's spawn path structurally lacks the parent event channel, rather than
  relying on a runtime check to suppress notifications.

## Key code references

- propose_task tool: `crates/phoenix-tools/src/propose_task.rs`
- interception + `resolve_task_file` + approval transitions:
  `crates/phoenix-state-machine/src/transition.rs` (`handle_core_llm_response`)
- approval executors: `crates/phoenix-ide/src/runtime/executor.rs`
  (`execute_approve_task`, `execute_approve_task_fresh_handoff`)
- handoff plumbing: `crates/phoenix-ide/src/runtime.rs`
  (`TaskApprovalHandoffRequest` / `TaskApprovalHandoffResponse`)
- fire-and-forget template: `crates/phoenix-ide/src/chain_qa.rs`
  (`ChainQa::submit_question`)
- ConvMode + registry selection: `crates/phoenix-core/src/domain/db_schema.rs`,
  `crates/phoenix-ide/src/runtime.rs`, `crates/phoenix-tools/src/lib.rs`
