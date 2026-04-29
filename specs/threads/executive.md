# Phoenix Threads — Executive Summary

## Requirements Summary

Phoenix Threads makes a tree of related conversations queryable as a unit.
A user who has run a stream of work — e.g., conv #41 continued into #42
continued into #44, with kickstart-spawned branch #51 (continued into #52)
exploring a related sub-direction — can navigate to that whole tree as a
*thread* and ask recall questions ("where did we leave off?", "what
optimizations did we apply?") whose answers see the full thread, main line
and branches alike. Q&A history persists per thread so users can return
and review. From any answer, the user can kickstart a new conversation
that inherits the answer as seed content and joins the thread as a branch.
Kickstart is for *new direction in the same topic*, distinct from the
existing "continue where we left off" action which extends the main line.
Threads emerge automatically from continuation and kickstart lineage — no
manual grouping action — and standalone conversations remain ungrouped.

## Technical Summary

Threads are a derived primitive over two existing graph edges on
`conversations`: `continued_in_conv_id` (main-line continuation chain)
and `seed_parent_id` (kickstart branch attachment). No `threads` table
is introduced. Membership is computed by hybrid recursive CTE walks
(chain back/forward, plus a single seed-pointer lookup for branch
attachment). Q&A persists in a new `thread_qa` table (one row per
question/answer pair, indexed by `root_conv_id`). Each Q&A invocation
receives bundled context covering all thread members — for non-leaf
members, the trailing `MessageType::Continuation` summary on each member
itself; for leaves (main-line leaf and each branch leaf), the transcript
or an on-demand summary, with per-leaf in-memory caching. Prior Q&A
history is never fed back to the model, bounding cost and preventing
drift. Token streaming reuses the existing SSE infrastructure on a
thread-scoped channel with per-question discriminator. Kickstart reuses
the seeded-conversations mechanism (`seed_parent_id`, `seed_label`,
review-first draft prompt) with the Q&A answer as draft source. Q&A
model is Claude Sonnet 4.6.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-THR-001:** Recall Past Work Without Re-Explaining Context | ❌ Not Started | Headline benefit; satisfied jointly by REQ-THR-002 through REQ-THR-006 |
| **REQ-THR-002:** Conversations Form Threads as Trees of Related Work | ❌ Not Started | Tree membership: continuation chain (main line) + seed-pointer branches; sidebar groups hierarchically |
| **REQ-THR-003:** Thread Page as a Navigable Place | ❌ Not Started | New route `/threads/:rootConvId` |
| **REQ-THR-004:** Ask the Thread, Get a Streamed Answer | ❌ Not Started | Sonnet 4.6; reuses SSE token-stream infrastructure |
| **REQ-THR-005:** Q&A History Persists Per Thread | ❌ Not Started | New `thread_qa` table |
| **REQ-THR-006:** Consistent Quality As Q&A Accumulates | ❌ Not Started | Stateless per-question invocation; no prior Q&A in context |
| **REQ-THR-007:** Kickstart a New Conversation From an Answer | ❌ Not Started | Reuses REQ-SEED-001 |
| **REQ-THR-008:** Kickstart Adds a Branch to the Thread, Not a Continuation | ❌ Not Started | Reuses REQ-SEED-003 / REQ-SEED-004; new conv joins source thread as branch member |

## v1 (MVP) Scope

All eight requirements ship together. The user story is internally
consistent only when navigation, Q&A, persistence, and kickstart are all
present — partial delivery would expose a half-feature (e.g., navigation
without Q&A, or Q&A without a way to act on the answer). Sub-milestones
inside v1 are tracked as tasks under `tasks/`.

## Out of Scope (Tracked for Future)

- Post-hoc thread membership editing
- Thread renaming (v1 uses root conversation title)
- Q&A editing and deletion
- Follow-up Q&A that references prior Q&A as model context
- Cross-thread linking beyond the single kickstart breadcrumb
- Project-level summary or steering doc (separate, explicitly-deferred concept)
- Allium behavioral spec for thread Q&A lifecycle (pre-token / streaming / persisted, mid-stream failure, concurrent Q&A across tabs). The lifecycle has enough states to warrant a `.allium` distillation — recommended as a follow-up after v1 ships and the actual transitions are observable
