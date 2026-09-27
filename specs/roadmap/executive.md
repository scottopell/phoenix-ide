# Delivery Roadmap — Executive Summary

## Scope and Boundary

This spec governs the phoenix-ide delivery roadmap Issue: the v2 record
protocol posted as Issue comments and the reducer that projects those records
into the Issue body.

**In scope:**
- Record kinds, actor roles, and per-kind merge semantics
- Milestone-first Markdown projection, freshness, and acknowledgement
- The GitHub Actions workflow that runs the reducer, and the local validator

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
| **REQ-ROADMAP-001:** One generated roadmap Issue per project | Implemented | `run` reads the Issue from `PHOENIX_ROADMAP_ISSUE_NUMBER` and writes only when the body carries `ACTIVATION_MARKER` and `PHOENIX_ROADMAP_COORDINATOR_HARNESS` is set. |
| **REQ-ROADMAP-002:** Records are single immutable fenced comments | Implemented | `parseRecordComment`, `reduceComments`. |
| **REQ-ROADMAP-003:** Every record declares its actor role and harness | Implemented | `validateRecord` enforces `PERMISSIONS` and the coordinator harness. |
| **REQ-ROADMAP-004:** Outcomes are the roadmap backbone | Implemented | `APPLY.outcome`, `APPLY["outcome-retire"]`. |
| **REQ-ROADMAP-005:** Identifiers are unique and references resolve | Implemented | `claimId` and the `live*` lookups. |
| **REQ-ROADMAP-006:** Milestones separate required from optional outcomes | Implemented | `APPLY.milestone`, `renderMilestone`. |
| **REQ-ROADMAP-007:** Gates are single-use and clearing is permanent | Implemented | `APPLY.gate`, `APPLY["gate-clear"]`, `APPLY.decision` (scope-bound clearing). |
| **REQ-ROADMAP-008:** Decisions are immutable and supersede explicitly | Implemented | `APPLY.decision` (scope-overlapping supersession). |
| **REQ-ROADMAP-009:** Evidence is version-bound and per surface | Implemented | `surfaceDelivery` / `componentDelivery`; PR state from `githubApi().getPull`; URL rules in `githubUrl`. |
| **REQ-ROADMAP-010:** Status carries next action and execution pointers | Implemented | `APPLY.status`. |
| **REQ-ROADMAP-011:** Freshness and unknowns are explicit | Implemented | `age`, `deliveryCell`. |
| **REQ-ROADMAP-012:** Milestone-first projection | Implemented | `renderRoadmap`. |
| **REQ-ROADMAP-013:** Projection runs without a triggering record | Implemented | `.github/workflows/roadmap-issue-reducer.yml` schedule, dispatch, and concurrency group. |
| **REQ-ROADMAP-014:** The projection is the authoritative acknowledgement | Implemented | `snapshot-through` and `ack-window-from` markers with the in-window rejection list; best-effort reactions; `--validate`. |

Code: `scripts/roadmap-issue-reducer.mjs`. Tests:
`scripts/roadmap-issue-reducer.test.mjs`, with fixtures for a cleared user hold,
a changed PR head, a PR closed without merge, unverified PR state, partial
multi-PR delivery, continued ownership, split web/native delivery, an optional
outcome that must not block its milestone, acknowledgement overflow, reaction
failures, and activation before the Issue switch.

### Activation

Activation is a hard cutover to Issue #806; v1 records are rejected. #806's body
carries the activation marker; #651's does not, so the v2 reducer never writes
#651 even if it reaches `main` before the Issue variable changes. Cutover:
post the reviewed seed records on #806, merge, set
`PHOENIX_ROADMAP_COORDINATOR_HARNESS=phoenix@primary` and
`PHOENIX_ROADMAP_ISSUE_NUMBER=806`, dispatch the workflow once, then close #651.
Until both variables are set, runs report "not activated" and write nothing.

## Posting a record

Validate first, then post a comment consisting only of the fenced record:

```bash
node scripts/roadmap-issue-reducer.mjs --validate record.json   # or pipe on stdin
```

The validator checks structure and role; references to outcomes, gates, and
decisions are checked when the Issue is reduced.

After posting, re-read the Issue body. With `S` = `snapshot-through` and `W` =
`ack-window-from`: a comment ID above `S` is not yet processed; between `W` and
`S` it is accepted unless listed under "Recent rejections"; below `W` the body
makes no claim (check the reaction or re-post).

### Canonical examples

Worker — status (check-in, next action, pointers that may be machine-local):

```json
{ "kind": "status", "version": 2, "actor": { "role": "worker", "harness": "phoenix@devmbp" },
  "outcome": "sidebar-product-actions", "next": "Final exact-head qualification",
  "pointers": [{ "label": "#777", "url": "https://github.com/scottopell/phoenix-ide/pull/777" },
               { "label": "owner transcript", "url": "http://devmbp:8031/c/example", "harness": "phoenix@devmbp" }] }
```

Worker — evidence (`implemented`, `qualified`; `merged` is derived from GitHub):

```json
{ "kind": "evidence", "version": 2, "actor": { "role": "worker", "harness": "codex" },
  "outcome": "question-forms", "surface": "web", "stage": "qualified", "result": "pass",
  "subject": { "pr": 770, "head": "70202823f813ed55178dc5412b7bddce04b63400" },
  "url": "https://github.com/scottopell/phoenix-ide/pull/770/checks" }
```

Deployment evidence must link a GitHub receipt:

```json
{ "kind": "evidence", "version": 2, "actor": { "role": "coordinator", "harness": "phoenix@primary" },
  "outcome": "auto-continue", "surface": "server", "stage": "deployed", "result": "pass",
  "subject": { "target": "prod@primary", "commit": "02488dde1b1da76555816c2745d42f1ad4234f9a" },
  "url": "https://github.com/scottopell/phoenix-ide/pull/807#issuecomment-5859803423" }
```

Worker — gate and gate-clear (owner/external gates):

```json
{ "kind": "gate", "version": 2, "actor": { "role": "worker", "harness": "phoenix@devmbp" },
  "id": "g-797-restart-budget", "blocks": { "outcome": "overload-recovery" },
  "clearer": "owner", "condition": "Restart/budget review findings resolved on #797" }
```

```json
{ "kind": "gate-clear", "version": 2, "actor": { "role": "worker", "harness": "phoenix@devmbp" },
  "gate": "g-797-restart-budget",
  "evidence": "https://github.com/scottopell/phoenix-ide/pull/797#pullrequestreview-1" }
```

User — decision (any harness may record it; quote the user's exact words):

```json
{ "kind": "decision", "version": 2, "actor": { "role": "user", "harness": "phoenix@primary" },
  "id": "d-tf-no-auto-continue", "statement": "Native auto-continue controls are not a TestFlight prerequisite",
  "scope": ["testflight-1", "native-auto-continue"],
  "quote": "TestFlight is next; native auto-continue controls definitely are not a blocker." }
```

Coordinator only — `outcome`, `outcome-retire`, `milestone`, `milestone-retire`;
see `specs/roadmap/requirements.md` REQ-ROADMAP-004 and REQ-ROADMAP-006 and the
seed bundle on the adoption PR for complete examples.
