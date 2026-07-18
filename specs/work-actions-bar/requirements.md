# Work Actions Bar: Requirements

## User Story

As a developer with a finished (or stuck) Work or Branch conversation, I need a clear,
unambiguous row of action verbs so I know what to do next — without having to decode status
badges or click a button twice to arm it.

## Scope

This spec governs **what the work actions bar shows and when**, and **what each verb means
to the user**. It is a composition surface: it selects which verb is primary and which verbs
are present, given inputs owned by sibling specs. It re-derives none of those inputs.

It does **not** govern:

- Terminal action git semantics (worktree deletion, diff snapshot, mode-dependent branch
  disposition, confirmation gate) → `work-lifecycle` spec (REQ-WL-001/002/003).
- PR status, explicit active-PR selection, any compatibility primary-PR projection, the CI
  check-state and feedback-freshness signals, and the auto-fix affordance → `pr-association`
  spec (`PrStatusView`, `PrCheckState`, `PrFeedbackFreshness`, `PrAutoFixAffordance`,
  `WorkScopePrStatusContract`, `WorkActionsPrAffordanceContract`).
- Action legality — the gate that permits a terminal action to fire → bedrock's `TaskResolved`
  rule (REQ-BED-029 for terminal-on-resolution; REQ-BED-031 for context-exhausted
  disposability).
- Diff viewer and browser viewer mechanics → `viewer_slot` spec (REQ-VS-003 for the diff
  viewer; REQ-VS-008 for the browser session).
- The browser session launcher's home → `work-scope-ui` spec (REQ-WSUI-004).

---

### REQ-WAB-001: Bar Visibility

WHEN a conversation's `conv_mode_label` is `"Work"` or `"Branch"`
AND the conversation is in a disposable, non-running condition — phase one of `idle`,
  `error`, or `context_exhausted`
THE SYSTEM SHALL render the work actions bar.

WHEN the conversation is in any other phase, or its mode is not `Work` or `Branch`
THE SYSTEM SHALL NOT render the work actions bar.

**Design:** The three eligible phases are exactly the disposable, non-running conditions of
bedrock's `TaskResolved` rule. `idle` is the ordinary resting state; `error` and
`context_exhausted` are stuck conditions from which the conversation cannot resume without
user action, yet must still be disposable so the user is never forced to coax out a successful
LLM turn just to clean up work whose outcome they already know (REQ-BED-029 for the
terminal-on-resolution legality, REQ-BED-031 for context-exhausted disposability). Any other
phase, or a non-Work/Branch mode, hides the bar: no terminal action is legal while the agent
is live, and the bar's verbs have no meaning outside Work and Branch.

---

### REQ-WAB-002: Responsive Three-Zone Presentation

THE SYSTEM SHALL organize available work actions into three logical zones:

**REVIEW zone** — always present when the bar is visible:
- `View Diff` — opens the diff viewer through the viewer slot (REQ-VS-003, fullscreen
  presentation). The committed diff is the artifact of the work; this is the REVIEW verb.

**RESOLVE zone** — the push-forward action; suppressed in stuck phases (REQ-WAB-005):
- The primary verb selected by `WorkDisposition` (REQ-WAB-004): `Address feedback`,
  `Merge on GitHub #N ↗`, `Open PR #N ↗`, or `Create PR on GitHub ↗`.
- Optionally a second, non-glowing link-out beside the primary: when the primary is
  `Address feedback` and the PR's checks are confirmed passing, the honest `Merge on GitHub #N ↗`
  link rides alongside so an open PR offers both "address the feedback" and "go merge it"
  at once. This secondary is never a glowing primary (REQ-WAB-003).

**FINISH zone** — terminal verbs:
- `Clean up` — calls `mark-merged`. Its git effects (worktree deletion, mode-dependent branch
  disposition) are owned by `work-lifecycle` REQ-WL-002.
- `Abandon` — initiates the abandon flow: a diff snapshot and a confirmation dialog precede
  the same mode-dependent git effects, per `work-lifecycle` REQ-WL-001.
- One of the two terminal verbs may be suppressed in specific `WorkDisposition` cases
  (REQ-WAB-004).

WHEN the conversation is presented in a narrow mobile viewport
THE SYSTEM SHALL replace the persistent multi-zone action bar with one thin horizontally scrollable
rail containing the actionable associated PRs and their current status.
THE SYSTEM SHALL keep the transcript dominant while the rail is collapsed.

WHEN a desktop conversation has multiple actionable associated PRs and its active PR is either
actionable or absent
THE SYSTEM SHALL replace the persistent multi-zone action bar with a PR rail whose entries show the
PR number, title, branch, open-or-draft status, review state, and actionable feedback freshness.
Review state SHALL use the same approved and in-progress symbols and accessible labels as the
conversation sidebar. Review state SHALL appear on every PR entry, independently of which PR is
active and independently of fresh-comment counts.
WHEN a desktop conversation has fewer than two actionable associated PRs, its active PR cannot be
represented by the PR-selector rail, or PR metadata is still loading
THE SYSTEM SHALL preserve the StateBar active-PR selector fallback and present the derived Work
Actions verbs in one thin, horizontally scrollable rail. The system SHALL NOT render a larger
intermediate action bar while PR metadata loads. Loading, single-PR, terminal-PR, and no-PR states
SHALL use the same compact rail geometry so metadata refresh does not cause a large layout shift.

WHEN the user activates a PR in the rail
THE SYSTEM SHALL make that PR the explicit active PR through `pr-association` and expand an action
region upward from the rail. The expanded region SHALL present the active PR's single hero action
separately from its supporting context actions. Closing or switching the expanded region changes
only presentation; it does not create a parallel active-PR selection. The StateBar active-PR selector
SHALL be hidden only while the Work Actions rail can represent and owns that selection.

---

### REQ-WAB-003: Single Primary Across the Entire Bar

THE SYSTEM SHALL render exactly one button as the glowing primary at any time across the action
surface — or, in the continuation case, no primary at all.

WHEN `WorkDisposition` produces a RESOLVE verb (idle phase only), that verb is the primary.
WHEN `WorkDisposition` suppresses RESOLVE (stuck phases) or there is no push-forward action
(merged / closed / no-PR / gh-unavailable), the primary collapses to a FINISH verb
(`Clean up` or `Abandon`).
WHEN dirty no-PR work has no honest PR-creation link, `View Diff` in the REVIEW zone is the
primary so the user reviews the work before any terminal cleanup.
WHEN `WorkDisposition` is `continued`, there is no primary verb and all terminal verbs are
suppressed (REQ-WAB-009).

A RESOLVE disposition may additionally carry a single **secondary** link-out (the
`Merge on GitHub #N ↗` link beside an `Address feedback` primary). The secondary is structurally distinct from
the primary and never glows; it exists only alongside an `address_feedback` primary. This does
not violate the single-primary rule: there is still exactly one glowing button.

In the active-PR rail, the expanded region SHALL emphasize exactly one hero action. Supporting
review, link-out, cleanup, and abandon controls SHALL remain visually secondary. Cleanup SHALL
never be inferred as the rail's hero action from repository structure alone; when legal, it remains
available only among supporting context actions.

**Design:** A single glowing button is the user's answer to "what do I do next?" The
presentation carries the primary as a single slot selector (REVIEW / RESOLVE / Clean up /
Abandon / none), so two glowing buttons are structurally unrepresentable rather than forbidden
by a runtime check. The secondary link is a separate, non-primary slot — present-or-absent,
never glowing — so it cannot collapse into a second primary.

---

### REQ-WAB-004: WorkDisposition Derivation

THE SYSTEM SHALL derive a single `WorkDisposition` value from the conversation's phase, mode,
`continued_in_conv_id`, the explicit active PR selected by `pr-association`, `PrStatusView`
(from `pr-association`, including its `check_state` and `feedback_freshness` fields), and
`PrAutoFixAffordance` (from `pr-association`). The `WorkDisposition` selects the primary verb and
which verbs are present.

The derivation is evaluated top-to-bottom; the first matching row wins. It is **total**: every
reachable combination of phase, continuation, and PR state maps to exactly one row.

| # | Condition | WorkDisposition | Primary verb | Secondary / suppressed |
|---|---|---|---|---|
| 1 | `continued_in_conv_id` set | `continued` | none | RESOLVE + FINISH suppressed; muted note |
| 2 | phase ∈ {error, context_exhausted} | `stuck` | `Clean up` or `Abandon` per FINISH sub-table | RESOLVE suppressed |
| 3 | idle, PR open, message channel available | `address_feedback` | **Address feedback** (RESOLVE) | Clean up suppressed; `Merge on GitHub #N ↗` secondary when `check_state = passing` and refresh is fresh; otherwise `Open PR #N ↗` secondary when a PR URL is available |
| 4 | idle, PR open, `check_state = passing`, affordance disabled | `merge_ready` | **Merge on GitHub #N ↗** (RESOLVE, GitHub link) | Clean up suppressed |
| 5 | idle, PR open/draft, no other RESOLVE matched (draft, or affordance-disabled and not passing) | `pr_open_other` | **Open PR #N ↗** (RESOLVE, GitHub link) | Clean up suppressed |
| 6 | idle, PR merged | `clean_up_merged` | **Clean up** (FINISH) | — |
| 7 | idle, PR closed unmerged | `pr_closed` | **Abandon** (FINISH) | Clean up suppressed; note |
| 8a | idle, no PR found, refresh ≠ unavailable, work-change state = clean | `no_pr_clean` | **Clean up** (FINISH) | — |
| 8b | idle, no PR found, refresh ≠ unavailable, work-change state = dirty and PR-ready | `no_pr_create_pr` | **Create PR on GitHub ↗** (RESOLVE, GitHub link) | Clean up suppressed; note |
| 8c | idle, no PR found, refresh ≠ unavailable, work-change state dirty-needs-review / loading / unavailable | `no_pr_review` | **View Diff** (REVIEW) | Clean up suppressed; note |
| 9 | idle, gh unavailable (no PR identity, refresh = unavailable) | `gh_unavailable` | **Clean up** (FINISH) | warning note; single click |

The **Address feedback** affordance is enabled when Phoenix can post an auto-fix message to
the conversation: the conversation has a live message channel and the PR is open
(`PrAutoFixAffordance`, `pr-association`). A draft PR is never addressable. A degraded or
stale refresh changes the secondary link-out from `Merge on GitHub` to `Open PR`; it does not
replace the primary with a link under the user's pointer.

Every PR-specific verb or marker the bar renders SHALL identify and target the same explicit
active PR supplied by `pr-association`. The bar SHALL NOT silently choose among multiple
associated actionable PRs on its own, and SHALL NOT treat any compatibility singular primary-PR
projection as authority over an explicit pinned or inferred active selection.

The **FINISH sub-table** is a single shared selector (used by `stuck` and by the idle FINISH
rows), total over PR state:

| PR state | FINISH primary | Inline note (stuck path) |
|---|---|---|
| merged | `Clean up` | — |
| closed unmerged | `Abandon` | — |
| open / draft | `Abandon` | "PR #N still open — merge on GitHub, or abandon." |
| no PR found (refresh ok) | `Clean up` | — |
| gh unavailable | `Clean up` | "gh unavailable — manual cleanup." |

Rows 3–5 together cover **every** idle open-or-draft PR. Address feedback is the primary on
every reachable open PR — not gated on failing checks, refresh availability, or a prior
feedback-freshness signal — because review comments may need addressing whether checks pass or
fail and whether or not a freshness baseline has yet been seeded; the freshness and coverage
signals ride as markers on the button rather than gating its presence. Keeping Address feedback
primary for stale cached open-PR snapshots prevents the primary action from changing under the
pointer when the async fresh-status request completes. When checks are confirmed passing on a
fresh refresh, the honest `Merge on GitHub #N ↗` link rides alongside as the non-glowing
secondary; when mergeability is not freshly confirmed, `Open PR #N ↗` is the non-glowing
secondary. The Merge link is the primary only when the PR cannot be addressed (no message
channel). The honest-label rule applies: rows 4 and 5 both open GitHub, but only a passing PR is
labelled "Merge on GitHub"; a non-passing open PR (or any draft) is labelled "Open PR" so the
bar never promises a merge the checks do not support. Both labels carry the ↗ external-navigation
glyph and open GitHub in a new tab — neither performs the merge in Phoenix (REQ-WAB-010).

Rows 8a–8c split no-PR work by the typed work-change summary. Clean no-PR work is terminal and may clean up. Dirty no-PR work is not terminal: if the local committed work is pushed to a matching GitHub remote branch, the bar may offer an honest `Create PR on GitHub ↗` link; otherwise `View Diff` is the safe primary. Uncommitted changes, unpushed commits, remote divergence, unknown remote state, and non-GitHub remotes all route to `View Diff`. The bar never makes commit, push, merge, or terminal automation the hero.

bar. The bar re-derives the disposition on every render. The `check_state` and
`feedback_freshness` signals are read as typed fields on `PrStatusView`, never reconstructed
from the raw provider wire.

---

### REQ-WAB-005: RESOLVE Zone Suppressed in Stuck Phases

WHEN the conversation phase is `error` or `context_exhausted`
THE SYSTEM SHALL NOT render the RESOLVE zone (no Address feedback, no PR link).

**Design:** Address feedback posts a `UserMessage` to the conversation. In `error` or
`context_exhausted`, the conversation cannot resume a new LLM turn from a user message — the
message would be rejected or would silently reopen a non-resumable stuck state. The only safe
actions from a stuck conversation are terminal cleanup verbs, so RESOLVE is suppressed
entirely rather than shown disabled (REQ-WAB-008).

---

### REQ-WAB-006: View Browser Exclusion

THE SYSTEM SHALL NOT render a `View Browser` button in the work actions bar.

The browser session affordance is owned by the `work-scope-ui` spec: the work scope reports
its browser session inventory (REQ-WSUI-004), and an active session is surfaced to the user
through the viewer slot's browser viewer (REQ-VS-008). It is a session affordance — it
surfaces a browser session the agent spawned — keyed to the work scope, not to the work
actions bar.

There is no "review the final app" slot in Phoenix; Phoenix does not launch a browser session
for the purpose of reviewing shipped work. The work actions bar REVIEW zone is scoped to diff
review only: the committed diff is the artifact of the work, and `View Diff` (REQ-VS-003) is
the appropriate REVIEW verb.

---

### REQ-WAB-007: Terminal Verb Tooltips

EACH terminal verb (`Clean up`, `Abandon`) SHALL carry an info-icon (ⓘ) tooltip that conveys
the verb's intent AND the key behavioral difference between the two verbs: Abandon captures a
diff snapshot first and requires a confirmation dialog before any destructive git operation;
Clean up does neither.

The tooltip copy SHALL be mode-sensitive, because branch disposition depends on mode (per
`work-lifecycle` REQ-WL-002: Work mode deletes the managed task branch, Branch mode keeps the
user's branch). Tooltip text:

**Clean up (Work mode):** "Mark as merged. Deletes the worktree and task branch. No
confirmation — use Abandon if you want a diff snapshot before deletion."

**Clean up (Branch mode):** "Mark as merged. Deletes the worktree; your branch is kept. No
confirmation — use Abandon if you want a diff snapshot before deletion."

**Abandon (Work mode):** "Capture a diff snapshot, then delete the worktree and task branch.
Requires confirmation."

**Abandon (Branch mode):** "Capture a diff snapshot and delete the worktree; your branch is
kept. Requires confirmation."

**Design:** For a given mode, Clean up and Abandon produce the **same** git side effects —
worktree deleted, task branch deleted (Work) or kept (Branch) — per `work-lifecycle`
REQ-WL-001/002. The real differences are Abandon's diff snapshot and its confirmation dialog.
The tooltips convey intent and that snapshot/confirm difference; they must not pretend the git
effects differ between the two verbs.

---

### REQ-WAB-008: No Disabled-as-Status, No Two-Step Toggle

THE SYSTEM SHALL NOT render a disabled button as a status display.

A disabled button reads as "this action is temporarily unavailable — try again later or fix a
prerequisite." Status belongs in the StateBar (the PR badge and phase indicator), never in the
work actions bar as a ghosted, un-clickable control. Every button the bar renders is enabled
and invocable; a verb that does not apply to the current state is absent, not disabled.

THE SYSTEM SHALL NOT use a click-to-enable-then-click-again affordance for any work actions
bar verb.

A button that arms itself on the first click and executes only on the second violates the
principle that each button has one fixed meaning. Confirmation for a destructive action is
handled by a dialog (Abandon), not by mutating a button's behavior across clicks. The
`gh_unavailable` disposition (REQ-WAB-004) makes this concrete: when gh cannot confirm a PR
and there is no PR identity, `Clean up` is an enabled, single-click primary with a note
explaining that cleanup proceeds without gh confirmation — not a disabled "waiting for PR
merge" control, and not a two-step enable-then-confirm toggle.

---

### REQ-WAB-009: Continuation Mute

WHEN the conversation's `continued_in_conv_id` is set
THE SYSTEM SHALL render the bar with no RESOLVE or FINISH verbs and no primary
AND SHALL show a muted inline note: "Continued — actions belong on the continuation."

The continuation is the live conversation; any terminal decision belongs there. bedrock
REQ-BED-031 also forbids terminal actions on a context-exhausted parent that has a
continuation, so the suppressed bar matches the server-side legality gate.

---

### REQ-WAB-011: Mobile Active-PR Rail

WHEN actionable associated PRs exist on mobile
THE SYSTEM SHALL render a thin PR rail directly above the conversation input that:

- contains only open or draft actionable PRs, never closed or merged history;
- identifies each PR by number and current state;
- marks the explicit active PR without silently selecting one;
- shows feedback freshness as a compact notification-style badge on its targeted PR;
- scrolls horizontally rather than wrapping into multiple persistent rows; and
- uses the full repository-plus-PR identity when selecting or expanding a PR.

WHEN the active PR is expanded
THE SYSTEM SHALL animate a non-modal action region into rows above the rail. The first row SHALL
contain the single hero action. A supporting row SHALL provide the legal review, GitHub link-out,
cleanup, and abandon controls without promoting cleanup as the suggested action. The action region
SHALL expose active-PR branch context and SHALL collapse when the active PR is activated again.

The rail SHALL NOT store a parallel active PR, infer by recency, show closed PRs as selectable, or
reinterpret a compatibility primary-PR projection as authoritative.

### REQ-WAB-012: Desktop Active-PR Rail

WHEN at least two actionable associated PRs exist on desktop and the rail can represent the explicit
active selection
THE SYSTEM SHALL render a persistent PR rail whose entries identify each PR by number, title,
branch, open-or-draft state, and targeted feedback freshness.

WHEN the user activates the explicit active PR
THE SYSTEM SHALL expand or collapse the active PR's hero and supporting action groups.
WHEN the user activates another PR
THE SYSTEM SHALL pin that PR through the shared `pr-association` selection authority before
expanding it.

The Work Actions rail and StateBar selector SHALL derive availability from the same rule. The
StateBar selector SHALL remain available while Work Actions is hidden, while fewer than two desktop
PRs are actionable, or when a terminal active PR cannot be represented by the rail.

---

### REQ-WAB-010: PR Link Verbs Open GitHub

WHEN `WorkDisposition` is `merge_ready` (verb `Merge on GitHub #N ↗`), `pr_open_other` (verb
`Open PR #N ↗`), or `no_pr_create_pr` (verb `Create PR on GitHub ↗`)
THE SYSTEM SHALL render the RESOLVE verb as a link that opens the PR's GitHub URL in a new
browser tab, NOT as a button that calls a Phoenix API.

Phoenix has no PR merge API. The ↗ glyph signals external navigation. Phoenix never merges or
pushes to origin on the user's behalf (`work-lifecycle` REQ-WL-002); the verb navigates the
user to GitHub to complete the merge through their normal PR workflow.
