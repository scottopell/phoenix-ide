# Delivery Roadmap — Executive Summary

## Scope and Boundary

This spec governs the phoenix-ide delivery roadmap Issue: the v2 record
protocol posted as Issue comments and the reducer that projects those records
into the Issue body.

**In scope:**
- Record kinds, actor roles, and per-kind merge semantics
- Milestone-first Markdown projection, freshness, and rejection reporting
- The GitHub Actions workflow that runs the reducer

**Explicitly out of scope:**
- Harness operations (runtime states, restarts, transcript mechanics)
- Requirements, product decisions, review state, and task status, which keep
  their existing authorities
- A roadmap feature for projects Phoenix works on

## Why It Exists

The v1 roadmap stored one free-text row per workstream, replaced whole by its
latest comment. Writers copied stale claims forward, holds could not be cleared
because they had no identity, merged and deployed work stayed listed as
unfinished, and a single Issue-wide marker made every row look fresh. Several
harnesses on different machines now contribute, so the roadmap must also avoid
references that only resolve on one machine.

## Current Reality

| Requirement | Status | Notes |
|---|---|---|
| **REQ-ROADMAP-001:** One generated roadmap Issue per project | Implemented | `run` reads the Issue from `PHOENIX_ROADMAP_ISSUE_NUMBER`. |
| **REQ-ROADMAP-002:** Records are single immutable fenced comments | Implemented | `parseRecordComment`, `reduceComments`. |
| **REQ-ROADMAP-003:** Every record declares its actor role and harness | Implemented | `validateRecord` enforces `PERMISSIONS`; coordinator harness from `PHOENIX_ROADMAP_COORDINATOR_HARNESS`. |
| **REQ-ROADMAP-004:** Outcomes are the roadmap backbone | Implemented | `APPLY.outcome`, `APPLY["outcome-retire"]`. |
| **REQ-ROADMAP-005:** Identifiers are unique and references resolve | Implemented | `claimId` and the `live*` lookups. |
| **REQ-ROADMAP-006:** Milestones separate required from optional outcomes | Implemented | `APPLY.milestone`, `renderMilestone`. |
| **REQ-ROADMAP-007:** Gates are single-use and clearing is permanent | Implemented | `APPLY.gate`, `APPLY["gate-clear"]`, `APPLY.decision`. |
| **REQ-ROADMAP-008:** Decisions are immutable and supersede explicitly | Implemented | `APPLY.decision`. |
| **REQ-ROADMAP-009:** Evidence is version-bound and per surface | Implemented | `surfaceDelivery`; PR state from `githubApi().getPull`. |
| **REQ-ROADMAP-010:** Status carries next action and execution pointers | Implemented | `APPLY.status`. |
| **REQ-ROADMAP-011:** Freshness and unknowns are explicit | Implemented | `age`, `deliveryCell`. |
| **REQ-ROADMAP-012:** Milestone-first projection | Implemented | `renderRoadmap`. |
| **REQ-ROADMAP-013:** Projection runs without a triggering record | Implemented | `.github/workflows/roadmap-issue-reducer.yml` schedule, dispatch, and concurrency group. |
| **REQ-ROADMAP-014:** The projection is the authoritative acknowledgement | Implemented | Snapshot marker and "Recent rejections" section; reactions via `githubApi().setReaction`. |

Code: `scripts/roadmap-issue-reducer.mjs`. Tests:
`scripts/roadmap-issue-reducer.test.mjs`, which include fixtures for a cleared
user hold, a changed PR head, continued ownership, split web/native delivery,
and an optional outcome that must not block its milestone.

Activation is a hard cutover to a new Issue: the v1 Issue #651 is closed once
the repository variable points at the new Issue. v1 records are rejected.

## Posting a record

Post a comment on the roadmap Issue consisting only of:

````markdown
```phoenix-roadmap
{ "kind": "status", "version": 2,
  "actor": { "role": "worker", "harness": "phoenix@devmbp" },
  "outcome": "parallel-work", "next": "Qualify exact head on CI" }
```
````

Then re-read the Issue body: the record is accepted once the
`phoenix-roadmap:snapshot-through` marker is at or above the comment ID and the
comment is not listed under "Recent rejections".
