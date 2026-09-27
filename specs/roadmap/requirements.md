# Delivery Roadmap

## Scope

The phoenix-ide delivery roadmap is one GitHub Issue whose body is generated
from structured records posted as comments on that Issue. It answers, for the
phoenix-ide project: which user-visible outcomes are committed, which milestones
they serve, who is accountable, what blocks them, how far each has been
delivered, and how fresh that knowledge is.

The roadmap is **project-specific and harness-neutral**. It tracks outcomes of
the Phoenix product. Any harness able to use the GitHub API may contribute —
each Phoenix instance, Codex, Claude Code — and no contributor needs another
harness's local state to read or write it. How any harness operates (runtime
states, stalls, restarts, transcript mechanics) is out of scope; the roadmap
sees only the effects of that work: evidence, status, gates, and decisions.

The roadmap is a coordination authority only. Requirements remain in specs and
Allium, decisions about the product in ADRs, review state in PRs, task status in
taskmd filenames, and shipped behavior on `main`.

## User Story

As the user directing several agent harnesses on different machines, I open one
Issue and see, milestone first, which outcomes are delivered, which gate each is
waiting on, who owns clearing it, and whether anyone has checked in recently.
When a hold is cleared, a PR head changes, or a feature ships on one surface
but not another, the page reflects it without anyone rewriting other people's
entries, and a stale or unknown fact looks stale or unknown rather than current.

## Requirements

### REQ-ROADMAP-001: One generated roadmap Issue per project

THE SYSTEM SHALL generate the entire projection of the configured roadmap Issue
body from the records in that Issue's comments.

THE SYSTEM SHALL identify the roadmap Issue by repository configuration, not by
a number embedded in the reducer.

THE SYSTEM SHALL project only into an Issue whose body carries the v2
activation marker, and only when a coordinator harness is configured. WHEN
either is missing THE SYSTEM SHALL write nothing and report that the roadmap is
not activated.

**Rationale:** The Markdown is an output. A body anyone edits by hand is
overwritten by the next projection and drifts from the records meanwhile. The
activation guard makes the reducer and the Issue selection independent writes:
deploying the reducer before the Issue selection changes, or failing to change
it, leaves a non-v2 Issue untouched.

---

### REQ-ROADMAP-002: Records are single immutable fenced comments

THE SYSTEM SHALL treat a trusted comment as a record only when its entire body
is one `phoenix-roadmap` fenced block containing one JSON object with
`version` 2 and a known `kind`.

THE SYSTEM SHALL reject, with a stated reason, a trusted comment that contains a
`phoenix-roadmap` fence together with other content, contains a retired v1
roadmap fence, fails validation, or has been edited after creation. A
correction is a new record.

THE SYSTEM SHALL ignore comments from authors outside the repository's owners,
members, and collaborators.

**Rationale:** Records are facts in an ordered log. Editing a record rewrites
history that later records were posted against; deleting one remains possible
as an administrative correction.

---

### REQ-ROADMAP-003: Every record declares its actor role and harness

THE SYSTEM SHALL require every record to carry `actor.role` (`coordinator`,
`worker`, or `user`) and `actor.harness`, a handle such as `phoenix@<machine>`,
`codex`, or `claude-code`.

THE SYSTEM SHALL accept each record kind only from these roles:

| Kind | Roles |
|---|---|
| `outcome`, `outcome-retire`, `milestone`, `milestone-retire` | coordinator |
| `gate`, `gate-clear`, `status` | coordinator, worker |
| `decision` | coordinator, user |
| `evidence` | coordinator, worker; `accepted` stage: coordinator, user |

THE SYSTEM SHALL reject coordinator records from any harness other than the
configured coordinator harness.

**Rationale:** Every contributor posts under the same GitHub account, so the
GitHub author cannot distinguish the coordinator, a worker, and the user. The
declared role bounds what a record can say; it is an auditable convention, not
a security boundary.

---

### REQ-ROADMAP-004: Outcomes are the roadmap backbone

THE SYSTEM SHALL model an outcome as an enduring user-visible improvement with
an identifier, title, intent, acceptance criteria, an accountable owner
harness, an order, and the delivery surfaces it spans (for example `server`,
`web`, `ios`).

THE SYSTEM SHALL let a later coordinator `outcome` record replace an earlier
one with the same identifier, and SHALL make `outcome-retire` (reason
`delivered` or `dropped`) permanent: later records naming a retired outcome are
rejected.

THE SYSTEM SHALL bound the number of live outcomes and reject new outcomes
beyond that bound.

---

### REQ-ROADMAP-005: Identifiers are unique and references resolve

THE SYSTEM SHALL keep outcome, milestone, gate, and decision identifiers in one
namespace and reject a record that reuses another kind's identifier.

THE SYSTEM SHALL reject a record that references an outcome, milestone, gate, or
decision not established by an earlier accepted record.

---

### REQ-ROADMAP-006: Milestones separate required from optional outcomes

THE SYSTEM SHALL model a milestone as an ordered set of required
`(outcome, surface, stage)` triples and an explicit list of optional outcomes;
an outcome cannot be both.

THE SYSTEM SHALL count a milestone requirement as met only when that outcome's
delivery on that surface has reached the required stage as defined by
REQ-ROADMAP-009.

THE SYSTEM SHALL never count gates on optional outcomes toward a milestone.

WHEN a coordinator milestone record changes an existing milestone's required
set
THE SYSTEM SHALL require it to cite a current decision scoped to that milestone.

WHEN a live milestone requires an outcome retired as `dropped`
THE SYSTEM SHALL surface that as needing the coordinator.

---

### REQ-ROADMAP-007: Gates are single-use and clearing is permanent

THE SYSTEM SHALL model a gate as an unmet condition blocking exactly one outcome
or milestone, naming who can clear it: `user`, `coordinator`, `owner`, or
`external`.

THE SYSTEM SHALL permit only the coordinator to gate a milestone.

THE SYSTEM SHALL clear a `user` gate only through a `user` decision, a
`coordinator` gate only through a coordinator or user decision or a coordinator
`gate-clear`, and an `owner` or `external` gate through any permitted
`gate-clear` carrying a GitHub evidence URL or a decision. The user outranks the
coordinator.

THE SYSTEM SHALL let a decision clear only a gate that it, or the gate's blocked
outcome or milestone, names in the decision's scope.

THE SYSTEM SHALL reject any record that would reopen a cleared gate, including a
new gate reusing its identifier. A condition that recurs is a new gate.

**Rationale:** Holds resurrected because free-text blocker lists were copied
forward. A gate that can only move from open to cleared cannot resurrect.

---

### REQ-ROADMAP-008: Decisions are immutable and supersede explicitly

THE SYSTEM SHALL model a decision as an immutable statement with a scope of
existing identifiers, optional superseded decisions, and optional gates it
clears.

THE SYSTEM SHALL require a decision posted with role `user` to quote the user's
words, so the decision is legible where the conversation it came from is not
reachable.

THE SYSTEM SHALL treat a decision as current until another decision supersedes
it, SHALL accept a supersession only between decisions sharing at least one
scope identifier, and SHALL surface a decision superseded by two different
decisions as needing the coordinator.

---

### REQ-ROADMAP-009: Evidence is version-bound and per surface

THE SYSTEM SHALL record evidence for an outcome's surface at one of the stages
`implemented`, `qualified`, `merged`, `released`, `deployed`, `accepted`, each
with a result (`pass` or `fail`), a github.com evidence URL, and a
stage-specific subject: a PR and head commit for `qualified`; a commit for
`merged`; a release and commit for `released`; a target and commit for
`deployed`.

THE SYSTEM SHALL require `deployed` and `accepted` evidence to link a GitHub
receipt — an Issue or PR comment or review, an Actions run, or a release —
because a commit link proves code identity, not that a deployment or acceptance
happened.

THE SYSTEM SHALL keep each surface's delivery independent, so one outcome can be
deployed on one surface while unknown on another.

THE SYSTEM SHALL treat evidence naming a PR (other than `released`, `deployed`,
and `accepted`) as describing one component of the surface, and the other
evidence as an assertion about the whole surface. A surface reaches a
component stage only when every live component has reached it; one merged PR
does not deliver a surface that still has an open one. A whole-surface
assertion keeps its target and commit identity, and open components beneath it
are shown as follow-ups.

THE SYSTEM SHALL derive each component's state from GitHub at projection time:
a merged PR is `merged` regardless of its qualification records; an open PR is
`qualified` only when its latest qualification at the PR's current head passed;
a PR closed without merge is shown as such and excluded from readiness; and a
PR whose state could not be read counts only as `implemented` and is shown as
unverified.

WHEN both pass and fail are recorded for the same PR head
THE SYSTEM SHALL show the latest result and surface the disagreement as needing
the coordinator.

---

### REQ-ROADMAP-010: Status carries next action and execution pointers

THE SYSTEM SHALL keep one latest status per outcome: the next action, an
optional note, and up to five execution pointers, each optionally labelled with
the harness it belongs to.

THE SYSTEM SHALL replace, not accumulate, execution pointers, so a transcript
continuation changes where to look without changing accountability.

WHEN a worker harness other than the outcome's accountable owner posts status
THE SYSTEM SHALL accept it and surface the ownership disagreement as needing the
coordinator.

---

### REQ-ROADMAP-011: Freshness and unknowns are explicit

THE SYSTEM SHALL show, per outcome, the age of its latest status, evidence, or
outcome gate, and mark ages beyond a fixed threshold as stale. A status record
that repeats the previous next action is a valid check-in.

THE SYSTEM SHALL show a surface with no accepted evidence as unknown, never as
not delivered.

THE SYSTEM SHALL state the projection time and whether PR state was verified.

---

### REQ-ROADMAP-012: Milestone-first projection

THE SYSTEM SHALL render, in order: live milestones with their required outcomes,
met count, open gates, optional outcomes and current decisions; live outcomes
outside any milestone; items needing the coordinator; outcomes and milestones
retired recently; and recent rejected records with their reasons.

THE SYSTEM SHALL render only open gates, and SHALL escape record text so it
cannot break the projection's tables or markers.

---

### REQ-ROADMAP-013: Projection runs without a triggering record

THE SYSTEM SHALL re-project on a fixed schedule and on manual dispatch, in
addition to record creation, edit, and deletion, so derived PR state and ages
stay current when no harness is active.

THE SYSTEM SHALL serialize projection runs.

---

### REQ-ROADMAP-014: The projection is the authoritative acknowledgement

THE SYSTEM SHALL record in the projection the highest trusted comment it
included and the first comment of a bounded acknowledgement window covering the
most recent record comments, and SHALL list every rejected record in that
window with its reason.

A record whose comment falls in the window is accepted exactly when it is not
listed. For a record before the window the projection makes no claim.

THE SYSTEM SHALL additionally mark a newly created record with bot reactions
(processing, accepted, rejected) on a best-effort basis: a reaction failure
never prevents or alters the projection, and a projection failure fails the run
without marking the record rejected.

THE SYSTEM SHALL provide a local validator that checks a record's structure and
role before it is posted.

**Rationale:** Reaction authorship is not visible to every harness, and a
superseded pending run can skip a reaction; the projection itself is readable by
every contributor, and a bounded window keeps it within the Issue size limit.
