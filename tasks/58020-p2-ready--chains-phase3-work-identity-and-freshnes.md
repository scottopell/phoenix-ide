# Chains Phase 3: work-identity dock facet (REQ-CHN-008) + chain_qa freshness column rename

Deferred work from the chains redesign. Two items:

## REQ-CHN-008 — work-identity facet on the chain work-scope dock

The chain page's work-scope dock currently surfaces only the scope's live
runtime resources (bash / tmux / browser) via `WorkScopeInventory`. REQ-CHN-008
adds the complementary "what unit of work is this" facet to the **same**
`work_scope_key` surface (no per-member fan-out):

- Work identity — worktree path, branch, base branch, and the task (id + title)
  for Managed work — sourced from the members' `ConvMode` git metadata.
- PR health — `display_state` (open / draft / merged / closed), checks, and
  feedback-freshness — sourced from the existing PR-status / feedback pipeline
  that drives the StateBar (`specs/projects/` REQ-PROJ-011/030/031), NOT from
  the PR-association record alone.

Constraints: must NOT be folded into `WorkScopeInventory` (its contract is a
full-snapshot projection over in-memory runtime registries; externally-polled PR
state and durable git metadata have a different freshness model). When the chain
has no managed work scope, indicate absence rather than rendering empty fields.
See `specs/chains/` REQ-CHN-008 and design.md "Work identity on the work-scope
dock".

## chain_qa snapshot -> freshness column rename

`specs/chains/design.md` documents the freshness markers as
`chain_members_at_answer` / `chain_messages_at_answer`, but the merged schema and
code use `snapshot_member_count` / `snapshot_total_messages` (plus the
`ChainSnapshot` struct and `NewChainQa` / `ChainQaRow` fields). Purely cosmetic,
no behavior change — but the name "snapshot" contradicts the design's own
insistence (REQ-CHN-005/009) that the answer is computed against LIVE content and
the integers are only an age-of-answer marker. Rename code (migration + struct
fields + bind/query sites) to match the spec, OR conversely settle on the
`snapshot_*` names in the spec. Touch points: `db/migrations.rs`,
`domain/db_schema.rs`, `db/lib.rs`, `chain_qa.rs`.
