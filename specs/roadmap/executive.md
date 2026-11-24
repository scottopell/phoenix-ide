# Delivery Roadmap — Retirement Status

## Current Reality

[ADR-262](../adrs/262_retire-delivery-roadmap-with-preserved-history.md)
supersedes ADR-069 and retires the entire roadmap protocol. The original
requirements remain in `requirements.md` as non-normative historical text.
Issue [#806](https://github.com/scottopell/phoenix-ide/issues/806) retains its final
projection and comments with an explicit historical-only notice.

| Requirement | Status | Notes |
|---|---|---|
| REQ-ROADMAP-001 | Retired | No generated coordination projection or activation contract. |
| REQ-ROADMAP-002 | Retired | No record posting or validation obligation. |
| REQ-ROADMAP-003 | Retired | No roadmap actor-role or harness obligation. |
| REQ-ROADMAP-004 | Retired | No roadmap outcome registry. |
| REQ-ROADMAP-005 | Retired | Historical identifiers remain evidence only. |
| REQ-ROADMAP-006 | Retired | No generated milestone aggregation. |
| REQ-ROADMAP-007 | Retired | Historical gates do not establish current holds. |
| REQ-ROADMAP-008 | Retired | Current instructions/ADRs retain their own authority. |
| REQ-ROADMAP-009 | Retired | Review/merge/deploy evidence stays with its owning authority. |
| REQ-ROADMAP-010 | Retired | Current task/PR and coordination records retain status. |
| REQ-ROADMAP-011 | Retired | Archive render time is not current delivery freshness. |
| REQ-ROADMAP-012 | Retired | Final projection preserved, not maintained. |
| REQ-ROADMAP-013 | Retired | Updater disabled; source workflow/reducer removed. |
| REQ-ROADMAP-014 | Retired | No new acknowledgement or reaction protocol. |

## Live cutoff evidence — 2026-11-16

Readback of workflow ID `330648486` reports `disabled_manually`. Both repository
variables `PHOENIX_ROADMAP_ISSUE_NUMBER` and
`PHOENIX_ROADMAP_COORDINATOR_HARNESS` were deleted. No in-progress or waiting
executions were present. Issue #806's activation marker was removed after this
cutoff; its archive notice and preserved projection matched the submitted body
on readback.

GitHub retains two stale run records from November 11 as `queued`, with zero jobs.
Normal and force-cancel APIs return HTTP 409, “has not been queued yet.” They
have no executing updater. Disabled workflow, absent variables and absent
activation marker prevent a later invocation from rewriting the archive. This
is not a claim that GitHub reports zero queued records.

The Issue remains open; closure is not the write-prevention mechanism. No comment
was deleted or added as a roadmap record. No old rows were bulk imported or
interpreted as completion, cancellation, permission or a fresh commission.

## Source and verification

The updater workflow, reducer, dedicated reducer tests and their CI invocation
are removed together. Those tests exercised only the retired reducer/protocol;
their deletion accompanies feature removal, not suppression of a live
regression. Unrelated CI and task validation remain.

AGENTS.md and the canonical development skill stop mandatory archive reading and
record posting. Active tasks lose those duties while preserving historical
commissions, limitations and evidence. Current user instructions,
specifications/Allium, ADRs, task filenames, PRs and exact deployed evidence
retain their authority. No replacement coordination dashboard is introduced.

Scoped task/spec/workflow and reference checks qualify source retirement;
Rust/UI builds are not relevant to this slice. PR review and CI remain required
before the source removal is merged.
