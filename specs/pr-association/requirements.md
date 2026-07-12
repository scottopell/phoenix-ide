# PR Association: WorkScope-Owned Pull Request Identity, Selection, and Feedback Freshness

## User Story

As a developer using PhoenixIDE, I need Phoenix to durably remember the pull requests associated
with the work I am doing, keep their status fresh without hammering GitHub, explicitly select
which PR PR-specific actions target, and tell me when review feedback has arrived since the last
time it handed that feedback to the agent — so I can act on real review activity without Phoenix
blocking ordinary work or guessing the wrong PR.

This spec describes Phoenix's WorkScope-owned plural-PR model: durable observed-branch history
feeds durable PR association history, and one explicit active PR targets PR-specific surfaces.
`executive.md` tracks which surfaces already implement that model and which compatibility paths
remain.

## Scope

This spec **owns**:

- **Observed-branch history** — durable, WorkScope-keyed history of the settled task-branch
  heads Phoenix observed for a conversation's worktree scope, used as candidate PR heads.
- **PR ↔ work-scope association** — durable, WorkScope-keyed history of the pull requests
  Phoenix has observed for those candidate branches.
- **Active-PR selection** — explicit selection of the one PR PR-specific status/action surfaces
  target, including the distinction between an inferred selection and a user-pinned one.
- **Compatibility projection** — a ranked singular PR projection retained only for compatibility
  surfaces that consume a singular view while explicit active-PR targeting remains authoritative.
- **PR status observation and refresh** — fresh / stale-not-found / unavailable refresh
  semantics for one explicit PR target, including refresh-by-number against durable PR identity so
  branch/HEAD drift does not strand the user.
- **The Address-CI auto-fix affordance derivation** — whether the "Address CI & comments"
  control is enabled or disabled, and which explicit PR it targets, derived from the active-PR
  status view.
- **PR feedback freshness and baseline** — the agent-facing advisory count marker (`{N} new`,
  `{N} edited`) near the Address-CI action, the baseline of what Phoenix last handed the agent,
  the bounded poll that gates full feedback fetches, and the orthogonal coverage-health signal.

This spec does **not** own:

- **Terminal-action git side effects and PR-merge-state-as-cleanup-gate** — abandon and mark-
  as-merged worktree/branch disposition, and using PR merge state to label the cleanup path.
  The `work-lifecycle` spec owns these. `work-lifecycle`'s REQ-WL-003 consumes the PR status
  this spec produces as its cleanup gate; this spec provides the status, work-lifecycle
  decides cleanup. **PR feedback freshness is never the branch-health signal** — PR merge
  state is, and it belongs to `work-lifecycle`.
- **Transition legality** — when a terminal action is permitted based on conversation state.
  That is bedrock's `TaskResolved` rule and `TerminalActionRequiresNoContinuation` invariant.
- **The work-actions-bar UI surface composition** — button labels, action zones, tooltips,
  disposition derivation. The `work-actions-bar` spec consumes this spec's auto-fix affordance
  result and renders it.

### Surface attribution: StateBar PR identity vs. work-actions-bar auto-fix

Two distinct surfaces consume this spec's PR identity, and they must not be conflated:

This targeting contract uses an explicit active PR rather than a hidden singular "primary PR":

- The **StateBar** and **work actions bar** must target the same explicit active PR when one is
  selected.
- The active PR may be **inferred** from durable branch and association facts, or **pinned** by
  the user.
- A compatibility primary-PR projection may exist for singular compatibility consumers, but it is
  not the hidden authority for active multi-PR actions.

- The **StateBar** renders the **PR badge / PR identity link** (`StateBarPrBadge`) — the PR
  number, title, and state. This is a StateBar concern.
- The **work actions bar** renders the **Address-CI auto-fix affordance**
  (`WorkActions.tsx` → `PrRemediationActions`). The StateBar has no auto-fix logic.

Both must target the **same** explicit active PR. The advisory freshness marker (REQ-PRA-001)
lives next to the Address-CI action on the work actions bar, not on the StateBar PR badge.

---

## Requirements

### REQ-PRA-000: Durable Branch-First Association and Explicit Active PR

WHEN Phoenix observes the settled HEAD branch of a WorkScope-backed worktree at a supported
reconciliation boundary
THE SYSTEM SHALL record that observation as durable branch history keyed by WorkScope and
  repository identity

THE SYSTEM SHALL treat observed task-branch history as the candidate set for pull-request
  discovery
AND SHALL NOT require the currently checked-out branch, `ConvMode.branch_name`, or one hidden
  singular primary PR to be the sole authority for discovery

THE SYSTEM SHALL retain discovered PR associations for the WorkScope even after the worktree later
  checks out another branch or the local branch is deleted

THE SYSTEM SHALL represent the PR targeted by PR-specific surfaces as an explicit active PR,
  structurally distinct from:
- the full associated-PR set
- the currently checked-out branch
- the owning task/worktree
- any compatibility primary-PR projection retained for older singular surfaces

THE SYSTEM SHALL distinguish an **inferred** active PR from a **pinned** active PR so automatic
  reconciliation cannot silently overwrite deliberate user intent

WHEN active-PR inference is ambiguous
THE SYSTEM SHALL leave PR-specific actions ambiguous or unavailable rather than silently choosing
  by recency alone

**Design:** Phoenix owns one worktree lifecycle, not one-PR-only work. Durable settled-branch
observation gives Phoenix a cheap, local, structurally honest source of candidate PR heads without
parsing commands, installing Git hooks, or exposing raw WorkScope construction to agents. The
active PR is explicit because PR-specific surfaces need a target whose provenance is visible:
user-pinned intent must outrank inference, and compatibility singular projections must not remain
an invisible authority once multiple deliverable PRs exist.

---

### REQ-PRA-000a: Active-PR Inference Order

WHEN no valid pinned active PR exists
THE SYSTEM SHALL infer the active PR in this order:
1. the unique actionable associated PR whose head matches the latest settled observed branch
2. otherwise the only actionable associated PR across the task
3. otherwise a still-valid prior inferred selection that does not contradict newer authoritative
   branch or PR facts
4. otherwise no active PR

THE SYSTEM SHALL keep merged and closed PRs as durable history
AND SHALL exclude them from the normal actionable choice when one open actionable PR is otherwise
  uniquely determined

THE SYSTEM SHALL define draft handling consistently with PR-specific action semantics that consume
  the active PR

**Design:** Inference should feel smart, not magical. Latest-settled-branch match is the strongest
cheap signal of user intent Phoenix can observe locally. The fallback to "only actionable PR"
keeps the common single-open-PR case frictionless. Retaining a prior inferred selection is
acceptable only when newer authoritative observations do not contradict it. Once multiple
actionable PRs remain plausible, silence is safer than a wrong silent retarget.

---

### REQ-PRA-000b: PR-Specific Surfaces Share One Explicit Target

WHEN Phoenix renders or executes a PR-specific surface for a Work or Branch conversation
THE SYSTEM SHALL target the same explicit active PR across:
- the StateBar PR identity
- PR-specific status and check-state reads
- PR feedback freshness and coverage reads
- the `Address CI & comments` / `Address feedback` action
- any PR-specific link-out or diff surface added by sibling specs

THE SYSTEM SHALL carry complete repository-plus-PR-number identity for that target
AND SHALL NOT persist or route a bare PR number as the full PR identity

**Design:** Users can tolerate plural PR history; they cannot tolerate one button addressing PR A
while another badge or freshness marker silently describes PR B. One explicit target per
interaction keeps multi-PR behavior understandable and testable.

---

### REQ-PRA-000c: Compatibility Primary Projection Is Non-Authoritative

WHEN Phoenix maintains a singular primary-PR projection for compatibility surfaces that consume a
singular view
THE SYSTEM SHALL derive that projection from the associated-PR set as a compatibility view
AND SHALL NOT let it silently override a valid explicit active-PR selection for PR-specific
  surfaces

**Design:** Some consumers still need a singular compatibility view, but leaving it as hidden
runtime authority would preserve the original bug class under a new name. Compatibility is a
projection, not ownership.

---

### REQ-PRA-001: PR Feedback Freshness Indicator

WHEN a Work or Branch conversation has an open associated pull request
AND PR feedback changed since Phoenix last captured agent-facing PR remediation context
THE SYSTEM SHALL show a compact advisory marker near the `Address CI & comments` Work Action
  carrying a count, SUCH AS `{N} new` (net-new feedback items) or `{N} edited` (baseline items
  whose content changed with no net-new items)

THE SYSTEM SHALL NOT use PR feedback freshness as the StateBar branch-health signal

THE SYSTEM SHALL NOT block cleanup, abandon, or ordinary conversation use based on PR feedback
  freshness

**Design:** Fresh review activity is useful exactly where the user asks the agent to address
feedback — beside the Address-CI action on the work actions bar. It is not branch health (PR
merge state, owned by `work-lifecycle`, is) and it carries no lifecycle authority: it never
gates cleanup, abandon, or ordinary use.

---

### REQ-PRA-002: Agent-Facing PR Context Baseline

WHEN Phoenix successfully captures PR remediation context for an associated pull request
THE SYSTEM SHALL record that successful capture as the baseline for agent-facing PR feedback
  freshness

THE SYSTEM SHALL store compact baseline data for the work scope and PR number: capture
  timestamp; pull request `updated_at` when available; stable feedback identities/fingerprints
  when available

WHEN classifying freshness
THE SYSTEM SHALL treat feedback as new when current feedback contains stable identities absent
  from the latest successful baseline

WHEN no successful baseline exists
THE SYSTEM SHALL NOT show a freshness count

**Design:** The baseline is what Phoenix actually handed the agent, not what GitHub contained
at some unrelated time. Freshness is the difference between current feedback and that last
successful hand-off, so a count of `new` comments is meaningful relative to what the agent has
already seen. With no baseline, there is nothing to be "new" relative to, so no count is shown.

---

### REQ-PRA-003: Bounded PR Feedback Refresh

WHEN refreshing routine PR status
THE SYSTEM SHALL keep the poll lightweight
AND SHALL NOT fetch all PR feedback surfaces unless gated by evidence that feedback may have
  changed (evidence includes pull request `updated_at` newer than the latest successful
  baseline, or an explicit remediation context capture)

WHEN a feedback surface cannot be read during freshness classification
THE SYSTEM SHALL report the gap as coverage health (REQ-PRA-004), not as a coarsened freshness
  advisory, so an incomplete fetch is never mistaken for fresh feedback
AND SHALL log the failure

**Design:** PR status is polled routinely; full review surfaces (review threads, review and
issue comments) are slower and rate-limit sensitive. Fetch them only when they can actually
change the advisory — gated by a cheap `updated_at` comparison against the baseline, or by an
explicit remediation capture. When a surface cannot be fetched, the freshness count is left
unclassified and the gap is surfaced separately as coverage health rather than folded into a
vague freshness state — keeping "feedback changed" and "we could not read all feedback" as
distinct, unambiguous signals.

---

### REQ-PRA-004: PR Feedback Coverage Health

WHEN classifying PR feedback freshness AND at least one feedback surface cannot be read
THE SYSTEM SHALL surface a coverage-health signal distinct from the freshness advisory,
  identifying the unreadable surfaces

THE SYSTEM SHALL distinguish a user-actionable auth gap (`auth_required` — the GitHub CLI is
  not authenticated, resolvable via `gh auth login`) from a transient gap (`incomplete`)

THE SYSTEM SHALL treat any freshness count shown alongside a coverage gap as a lower bound

THE SYSTEM SHALL NOT block cleanup, abandon, or ordinary conversation use based on coverage
  health

**Design:** Coverage health is orthogonal to freshness: freshness answers "did feedback
change?", coverage answers "could we read all of it?". Folding the two together makes an
incomplete fetch indistinguishable from genuinely-fresh feedback. Separating them lets the UI
both show an honest (lower-bound) count and offer the user a concrete fix when the gap is an
auth problem, while neither signal carries lifecycle authority.

---

## Acceptance Criteria Summary

| ID | The system must… |
|----|------------------|
| REQ-PRA-000 | Record durable settled-branch observations as candidate PR heads, retain plural PR association history by WorkScope, and model one explicit active PR distinct from compatibility singular projections. |
| REQ-PRA-000a | Infer the active PR by latest observed actionable branch match, then only-actionable fallback, then still-valid prior inference; otherwise leave selection unset rather than silently choosing by recency. |
| REQ-PRA-000b | Target the same explicit active PR across StateBar identity, PR status, freshness, Address-CI / Address feedback, and sibling PR-specific surfaces. |
| REQ-PRA-000c | Retain any singular primary-PR view only as a compatibility projection, never as hidden authority over an explicit active-PR selection. |
| REQ-PRA-001 | Show a compact freshness advisory (a `{N} new` or `{N} edited` count) near Address-CI when an open active PR's feedback changed since the last agent-facing capture; never use it as branch health or as a lifecycle gate. |
| REQ-PRA-002 | Record each successful remediation-context capture as the freshness baseline (timestamp, PR `updated_at`, stable feedback identities) for the targeted PR; classify feedback new when identities are absent from the latest baseline; show no count when no baseline exists. |
| REQ-PRA-003 | Keep routine PR polling lightweight; fetch full feedback surfaces only when `updated_at` or an explicit capture says they may have changed; on an unreadable surface, report a coverage gap (REQ-PRA-004) rather than coarsening freshness, and log. |
| REQ-PRA-004 | Surface coverage health distinct from freshness when a feedback surface can't be read; distinguish user-actionable `auth_required` from transient `incomplete`; treat any concurrent freshness count as a lower bound; carry no lifecycle authority. |
