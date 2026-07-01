# Worked Examples

Two features taken end-to-end.
The first is state-machine-complex and crosses the Allium gate; the second stays
entirely in the markdown layer, showing that spEARS is complete without Allium.
Examples carry intent better than rules — when in doubt, imitate these.

* * *

## Example 1: Task approval (crosses the Allium gate)

### 1. Discover, then write requirements

Discovery establishes that reviewers approve pending tasks, and that approving a
task with unmet preconditions would be dangerous.
That yields `specs/task-approval/requirements.md`:

```markdown
# Task Approval

## User Story

As a reviewer, I need to approve a pending task so that it can proceed, but only when it is
actually safe to do so.

## Requirements

### REQ-TA-001: Approve a Pending Task

WHEN a reviewer approves a pending task
THE SYSTEM SHALL mark the task approved and allow it to proceed

**Rationale:** Reviewers are the gate that lets work move forward; approval must be a
deliberate, visible act they trust.

---

### REQ-TA-002: Block Approval When Preconditions Are Unmet

IF a task's preconditions are not satisfied
THE SYSTEM SHALL NOT allow approval, and SHALL show which precondition failed

**Rationale:** A reviewer who approves unsafe work loses trust in the whole gate; the
system must make the unsafe case impossible, not merely discouraged.
```

### 2. Skeletal executive.md

```markdown
# Task Approval — Executive Summary

## Requirements Summary
Reviewers can approve pending tasks, and approval is blocked when preconditions fail.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| **REQ-TA-001:** Approve a Pending Task | ❌ Not Started | — |
| **REQ-TA-002:** Block Approval When Preconditions Are Unmet | ❌ Not Started | — |

**Progress:** 0 of 2 complete

## Open Questions & Future Directions
- Do reviewers approve in batches? If usage shows it, bulk-approval is a likely next requirement.
```

### 3. The gate fires YES

Task approval is a lifecycle with preconditions — squarely
state-machine-complex.
Hand off to Allium (`allium elicit`), producing `task-approval.allium` that
references the REQ-IDs:

```text
entity Task {
    status: pending | approved | rejected
    preconditions_met: Boolean

    transitions status {
        pending -> approved
        pending -> rejected
        terminal: approved, rejected
    }
}

-- models REQ-TA-001 and REQ-TA-002
rule ApproveTask {
    when: ApproveRequested(task)
    requires: task.status = pending and task.preconditions_met
    ensures: task.status = approved
}
```

`allium propagate` turns this into tests (which must fail first); then you
implement.

### 4. A design fork appears → Proposed ADR → Accepted

Mid-implementation a real fork surfaces: should an approved task be reversible?
Both answers are defensible, so it becomes a decision of record —
`specs/adrs/004_approval-is-irreversible.md`, opened as `Proposed`, flipped to
`Accepted` once the call is made:

```markdown
# ADR-004: Task Approval Is Irreversible

- **Status:** Accepted
- **Date:** 2026-06-28
- **Affects:** REQ-TA-001

## Context
Approved tasks proceed immediately to downstream work. Whether approval can be undone
shapes the state machine and what reviewers can rely on.

## Options considered
1. **Reversible approval** — reviewers can undo. Flexible, but downstream work may already
   have started, making "undo" a lie.
2. **Irreversible approval** — once approved, the task proceeds; mistakes are corrected by a
   new task, not by rewinding. Predictable; trustworthy for downstream consumers.

## Decision
Approval is irreversible. The transition graph has no `approved -> pending` edge.

## Consequences
- **Positive:** downstream consumers can trust an approved task will not be rescinded.
- **Negative:** a mistaken approval requires a compensating action, not an undo.
- **Neutral:** the Allium `transitions` block encodes this directly (no reverse edge).

## References
- `task-approval.allium` rule `ApproveTask`
- REQ-TA-001
```

Note what just happened: the *why* of irreversibility did not go into
`requirements.md` (which stays timeless and says only *what*) or the `.allium`
(which says *how*). It went to an ADR, where the path-dependent reasoning
belongs.

### 5. Implement, verify, update status

Code carries the suture; tests verify; the behavioral layer is checked:

```rust
// REQ-TA-001: approve a pending task (irreversible per ADR-004)
pub fn approve(task: &mut Task) -> Result<()> { … }
```

Available Allium drift-check tooling confirms the `.allium` matches the code;
[validation.md](validation.md) confirms the markdown layer (EARS intact,
ADR-004 present, no contradictions, status accurate).
Then `executive.md` flips `REQ-TA-001` and `REQ-TA-002` to `✅`.

* * *

## Example 2: CSV export (stays in the markdown layer)

A reporting feature: export a table to CSV. No states, no lifecycle, no ordering
hazard — the Allium gate fires **NO**. spEARS handles it completely without
Allium.

```markdown
### REQ-EX-001: Export the Current View as CSV

WHEN a user exports the current table view
THE SYSTEM SHALL produce a CSV with one row per visible record and a header row

WHEN the view is empty
THE SYSTEM SHALL produce a CSV containing only the header row

**Rationale:** Users move data into spreadsheets to analyze it their own way; an export
that silently drops rows or omits headers quietly corrupts their analysis.
```

You implement directly against `REQ-EX-001` (with `// REQ-EX-001` comments),
write tests from the two EARS clauses, and track status in `executive.md`. There
is no `.allium`, no formal behavioral layer — and nothing is missing.
If a real decision arises (say, which delimiter to use for locales that reserve
the comma), it earns an ADR; otherwise the markdown layer is the whole spec.

This is the common case.
Most features look like Example 2; Example 1’s machinery is for the complex
minority that earns it.
