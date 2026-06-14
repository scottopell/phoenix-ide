# PR Association: WorkScope-Owned Pull Request Identity and Feedback Freshness

## User Story

As a developer using PhoenixIDE, I need Phoenix to durably remember which pull request belongs
to the work I am doing, keep that PR's status fresh without hammering GitHub, point the
"Address CI & comments" action at the right PR, and tell me when review feedback has arrived
since the last time it handed that feedback to the agent — so I can act on real review activity
without Phoenix blocking ordinary work or guessing the wrong PR.

## Scope

This spec **owns**:

- **PR ↔ work-scope association** — durable, WorkScope-keyed history of the pull requests
  Phoenix has observed for a conversation's worktree (or conversation) scope.
- **Primary-PR derivation** — selecting the one PR a status/action surface should treat as
  authoritative for a scope, ranked by display state and tie-broken by provider update time.
- **PR status observation and refresh** — fresh / stale-not-found / unavailable refresh
  semantics, including refresh-by-number against the persisted primary so branch/HEAD drift
  does not strand the user.
- **The Address-CI auto-fix affordance derivation** — whether the "Address CI & comments"
  control is enabled or disabled, and which PR it targets, derived from the PR status view.
- **PR feedback freshness and baseline** — the agent-facing advisory marker (`new comments`,
  `{N} new`, `updated`) near the Address-CI action, the baseline of what Phoenix last handed
  the agent, and the bounded poll that gates full feedback fetches.

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

- The **StateBar** renders the **PR badge / PR identity link** (`StateBarPrBadge`) — the PR
  number, title, and state. This is a StateBar concern.
- The **work actions bar** renders the **Address-CI auto-fix affordance**
  (`WorkActions.tsx` → `PrRemediationActions`). The StateBar has no auto-fix logic.

Both must target the **same** primary PR. The advisory freshness marker (REQ-PRA-001) lives
next to the Address-CI action on the work actions bar, not on the StateBar PR badge.

---

## Requirements

### REQ-PRA-001: PR Feedback Freshness Indicator

WHEN a Work or Branch conversation has an open associated pull request
AND PR feedback changed since Phoenix last captured agent-facing PR remediation context
THE SYSTEM SHALL show a compact advisory marker near the `Address CI & comments` Work Action
  SUCH AS `new comments`, `{N} new`, or `updated`

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
THE SYSTEM SHALL NOT show `new comments`

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

WHEN full feedback surfaces are unavailable during freshness classification
THE SYSTEM SHALL degrade to no count or a coarse `updated` advisory
AND SHALL log the failure

**Design:** PR status is polled routinely; full review surfaces (review threads, check runs,
comment bodies) are slower and rate-limit sensitive. Fetch them only when they can actually
change the advisory — gated by a cheap `updated_at` comparison against the baseline, or by an
explicit remediation capture. When the full surfaces cannot be fetched, the advisory degrades
gracefully (coarse `updated`, or nothing) and the failure is logged rather than silenced.

---

## Acceptance Criteria Summary

| ID | The system must… |
|----|------------------|
| REQ-PRA-001 | Show a compact freshness advisory near Address-CI when an open PR's feedback changed since the last agent-facing capture; never use it as branch health or as a lifecycle gate. |
| REQ-PRA-002 | Record each successful remediation-context capture as the freshness baseline (timestamp, PR `updated_at`, stable feedback identities); classify feedback new when identities are absent from the latest baseline; show nothing when no baseline exists. |
| REQ-PRA-003 | Keep routine PR polling lightweight; fetch full feedback surfaces only when `updated_at` or an explicit capture says they may have changed; degrade and log when unavailable. |
