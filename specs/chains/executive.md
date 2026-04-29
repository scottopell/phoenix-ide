# Phoenix Chains — Executive Summary

## Requirements Summary

Phoenix Chains makes a continuation chain of conversations queryable
as a unit. A user who has run a stream of work — e.g., conv #41
continued into #42 continued into #44 — can give the chain a
recognizable name ("auth refactor"), find it nested under a
collapsible header in the sidebar, navigate to a dedicated chain page,
and ask it recall questions ("what optimizations did we apply?")
whose answers see every member of the chain. Q&A history persists per
chain with snapshot-staleness indicators on answers generated against
earlier chain states. Chains emerge automatically from the existing
continuation graph (no manual grouping); standalone conversations
remain ungrouped. Chains are linear in v1 — kickstart and offshoots
are deferred pending resolution of the worktree-ownership invariant
for peer conversations. The headline benefit is recall without
re-explaining: the user does not have to extend a long conversation
to ask a recall question, and does not have to start a fresh
conversation and re-supply scope.

## Technical Summary

Chains are a derived primitive over Phoenix's existing
`conversations.continued_in_conv_id` graph. The only schema change to
`conversations` is a single nullable `chain_name TEXT` column on chain
root conversations. One new table: `chain_qa` (one row per
question/answer pair, indexed by `root_conv_id`, with explicit
`status` enum tracking `in_flight` / `completed` / `failed` /
`abandoned` and two integer snapshot counters for staleness
comparison). Each Q&A invocation receives bundled context covering
all chain members — for non-leaf members the trailing
`MessageType::Continuation` summary on each member's tail; for the
leaf the transcript or an in-process summary. Prior Q&A history is
never fed back to the model, bounding cost and preventing drift. Token
streaming reuses Phoenix's existing SSE infrastructure on a
chain-scoped channel with per-question discriminator. A startup sweep
transitions stale `in_flight` rows to `abandoned`. Q&A invocations
use a mid-tier model balanced for cost and accuracy.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-CHN-001:** Recall Past Work Without Re-Explaining Context | ❌ Not Started | Headline benefit; satisfied jointly by REQ-CHN-002 through REQ-CHN-006 |
| **REQ-CHN-002:** Continuation Chains Surface as First-Class Entities | ❌ Not Started | Sidebar nesting + chain identity; derived from `continued_in_conv_id` |
| **REQ-CHN-003:** Chain Page as a Navigable Place | ❌ Not Started | New route `/chains/:rootConvId`; deep-linkable |
| **REQ-CHN-004:** Ask the Chain, Get a Streamed Answer | ❌ Not Started | Mid-tier model; reuses SSE token-stream infrastructure |
| **REQ-CHN-005:** Q&A History Persists Per Chain | ❌ Not Started | New `chain_qa` table with `status` enum and integer snapshot counters |
| **REQ-CHN-006:** Consistent Quality As Q&A Accumulates | ❌ Not Started | Stateless per-question invocation; no prior Q&A in context; visual independence on the Q&A panel |
| **REQ-CHN-007:** Chain Has a User-Editable Name | ❌ Not Started | New nullable `conversations.chain_name`; defaults to root title |

## v1 (MVP) Scope

All seven requirements ship together. The user story is internally
consistent only when the chain is identifiable (sidebar grouping +
editable name + chain page) and queryable (Q&A with persistence and
staleness indication). Sub-milestones inside v1 are tracked as tasks
under `tasks/`.

## Out of Scope (Tracked for Future)

- Kickstart action and offshoot (tree-shaped) chains. Deferred pending
  resolution of the worktree-ownership invariant for peer
  conversations (a `specs/projects/` concern). Named in
  `requirements.md` as a future direction.
- Resume as a first-class action. Sidebar nesting and chain page
  visual emphasis on the latest member suffice.
- Manual chain membership editing.
- Q&A editing and deletion.
- Follow-up Q&A that references prior Q&A as model context. Named
  v1.5 path: a "reply" affordance that pre-fills the input with a
  quoted snippet so the user's question becomes self-contained,
  preserving REQ-CHN-006's stateless contract.
- Cross-chain linking.
- Project-level summary or steering doc.
- Retrieval-backed Q&A architecture. Named future direction; trigger
  to pivot is bundling cost becoming painful at observed chain sizes
  or a product-level decision to introduce ambient memory across
  non-chain conversations.
- Allium behavioral spec for chain Q&A lifecycle (`in_flight` /
  streaming / `completed` / `failed` / `abandoned`, snapshot
  computation, concurrent Q&A across tabs). The lifecycle has enough
  states to warrant a `.allium` distillation — recommended as a
  follow-up after v1 ships and the actual transitions are observable.
