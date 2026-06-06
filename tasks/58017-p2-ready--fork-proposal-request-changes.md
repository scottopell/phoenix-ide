Build REQ-PROJ-037: "Request Changes" on a fork proposal promotes it into a fresh Explore conversation for agent-assisted iteration.

Context / why: a fork proposal is reviewed with only Approve / Dismiss today (REQ-PROJ-034). The proposer (a Work-mode origin) is decoupled and has no attached LLM (REQ-PROJ-035), so "request changes" feedback cannot be delivered to it. Option C (chosen): promote the snapshot into a new Explore conversation seeded with the brief + the user's change note, and iterate via the existing Explore propose/feedback loop (REQ-PROJ-004). Spec is locked in (requirements REQ-PROJ-037, projects/design.md "Request Changes -> Explore refinement", Allium rule ForkProposalPromotedToExplore + `promoted` status). This task is the implementation.

Prerequisite: REQ-PROJ-033..036 (the fork feature itself) must be implemented first — this builds on the fork proposal record, the review surface, and the spawn/worktree machinery.

Scope:
- Add the `promoted` resolution + `refinement` conversation reference to the ForkProposal model and its persistence (migration if the proposals table is already shipped).
- Add the `/proposals/:id/request-changes` endpoint (free-text change note) and the `Effect::PromoteForkToExplore` executor handler, run under TASK_APPROVAL_MUTEX.
- Executor flow: allocate a fresh top-level Explore conversation; create its worktree cut from main_ref; write the snapshot body as an UNCOMMITTED draft on the temp branch (TaskSource: taskmd vs plain, REQ-PROJ-006); set spawned_from_conversation_id = origin (non-live breadcrumb); seed the Explore agent context with brief body + change note only. Commit the `promoted { refinement }` resolution atomically with the surface update.
- UI: third "Request Changes" action in the fork review surface (alongside Approve/Dismiss), with a note input. Withdraw the affordance once resolved.
- Refinement then uses the unchanged Explore->Work gateway (TaskApprovalExecuted / TaskApprovalFreshHandoffExecuted) — no second fork proposal, no path back to the origin.
- Lifecycle: `promoted` is terminal for the proposal; persists through origin terminal cleanup; removed only on origin hard delete (extend the existing cascades). The refinement Explore conversation has an independent lifecycle.
- Propagate the Allium rule to tests (/allium:propagate on ForkProposalPromotedToExplore).

Decoupling invariant to preserve: the origin is the proposer, never the iterator — the promotion must not mutate or notify it.
