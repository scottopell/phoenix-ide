# Work Actions Bar — Technical Design

## Architecture Overview

The work actions bar is a pure composition layer. It derives all of its state from sibling
specs' contracts and re-derives nothing:

- **`PrStatusView`** (with its `check_state` and `feedback_freshness` fields) and
  **`PrAutoFixAffordance`** — supplied by `pr-association` via `useConversationPrStatus`
  (`WorkScopePrStatusContract`) and `WorkActionsPrAffordanceContract`.
- **Terminal action semantics** — owned by `work-lifecycle`; the bar calls
  `POST /api/conversations/:id/mark-merged` and `POST /api/conversations/:id/abandon-task`.
- **Diff viewer** — owned by `viewer_slot` (REQ-VS-003); the bar opens the diff viewer in
  fullscreen presentation through the viewer slot.
- **Browser session** — owned by `work-scope-ui` (REQ-WSUI-004) and surfaced by `viewer_slot`
  (REQ-VS-008); the bar does not render any browser affordance.
- **Action legality** — bedrock's `TaskResolved` preconditions (REQ-BED-029, REQ-BED-031) are
  enforced server-side by the mark-merged and abandon-task handlers; the bar does not
  re-implement the gate.

The bar's own logic is exactly one function: `deriveWorkDisposition`, a pure function of inputs
that selects one `WorkDisposition` variant and maps it to the button set and the single
primary slot.

## WorkDisposition Derivation

`WorkDisposition` is a derived enumeration, not stored state. Inputs:

| Input | Source |
|---|---|
| `phase` | bedrock phase (`idle` / `error` / `context_exhausted`) |
| `mode` | `Work` or `Branch` |
| `continued_in_conv_id` | `conversation.continued_in_conv_id` |
| `PrStatusView` | `useConversationPrStatus` → `prStatusHandle.state` |
| `PrAutoFixAffordance` | `WorkActionsPrAffordanceContract.derive_affordance(status)` |

Derivation is strict top-to-bottom; the first matching row wins. It is total — every reachable
input maps to exactly one row:

1. `continued_in_conv_id != null` → **continued**
2. `phase ∈ {error, context_exhausted}` → **stuck** (FINISH primary via the shared selector;
   RESOLVE suppressed)
3. `phase = idle`, PR `display_state = open`, `affordance.enabled` → **address_feedback**
   (carries a `merge_pr` secondary link when `check_state = passing`)
4. `phase = idle`, PR `display_state = open`, `check_state = passing`, `affordance` disabled →
   **merge_ready**
5. `phase = idle`, PR `display_state ∈ {open, draft}`, no other RESOLVE matched →
   **pr_open_other**
6. `phase = idle`, `display_state = merged` → **clean_up_merged**
7. `phase = idle`, `display_state = closed` → **pr_closed**
8. `phase = idle`, `pr = null`, `refresh.state != unavailable` → **no_pr**
9. `phase = idle`, `pr = null`, `refresh.state = unavailable` → **gh_unavailable**

Rows 3–5 partition every idle open-or-draft PR; there is no open-PR state that falls through.
Addressability reads `affordance.enabled` (`PrAutoFixAffordance`) and `display_state = open`
only — it does not gate on `check_state` or `feedback_freshness`, since review comments may
need addressing regardless of check state and a freshness baseline is seeded only after the
first address. The `merge_ready` predicate reads `PrStatusView.check_state` (a typed
`pr-association` field); the same field decides whether the `merge_pr` secondary link rides
beside the `address_feedback` primary. `feedback_freshness` and `feedback_coverage` drive
on-button markers, not the disposition split.

## The Single FINISH-Primary Selector

`finishPrimaryForDisposition(disposition, status)` is the **one** source of FINISH-primary
selection, shared by the `stuck` path and every idle FINISH row, so the PR-state logic is not
duplicated. It is total over PR state:

| PR state | FINISH primary |
|---|---|
| merged | `Clean up` |
| closed unmerged | `Abandon` |
| open / draft | `Abandon` (stuck only; idle open/draft is handled by rows 3–5 with a RESOLVE primary) |
| no PR found | `Clean up` |
| gh unavailable | `Clean up` |

The selector never returns the RESOLVE slot and never returns `none`; those are decided by the
disposition rows directly.

## Button Set and Primary per Disposition

`★` marks the single glowing primary; `—` marks a suppressed verb.

| Disposition | REVIEW | RESOLVE | Clean up | Abandon | Primary | Note |
|---|---|---|---|---|---|---|
| `continued` | View Diff | — | — | — | none | "Continued — actions belong on the continuation." |
| `stuck` (PR merged) | View Diff | — | ★ Clean up | Abandon | Clean up | — |
| `stuck` (PR closed) | View Diff | — | Clean up | ★ Abandon | Abandon | — |
| `stuck` (PR open/draft) | View Diff | — | Clean up | ★ Abandon | Abandon | "PR #N still open — merge on GitHub, or abandon." |
| `stuck` (no PR) | View Diff | — | ★ Clean up | Abandon | Clean up | — |
| `stuck` (gh unavailable) | View Diff | — | ★ Clean up | Abandon | Clean up | "gh unavailable — manual cleanup." |
| `address_feedback` | View Diff | ★ Address feedback (+ `Merge PR #N ↗` secondary link when checks pass) | — | Abandon | RESOLVE | — |
| `merge_ready` | View Diff | ★ Merge PR #N ↗ | — | Abandon | RESOLVE | — |
| `pr_open_other` | View Diff | ★ Open PR #N ↗ | — | Abandon | RESOLVE | — |
| `clean_up_merged` | View Diff | — | ★ Clean up | Abandon | Clean up | — |
| `pr_closed` | View Diff | — | — | ★ Abandon | Abandon | "PR closed — abandon to clean up." |
| `no_pr` | View Diff | — | ★ Clean up | Abandon | Clean up | — |
| `gh_unavailable` | View Diff | — | ★ Clean up | Abandon | Clean up | "gh unavailable — manual cleanup." |

The `stuck` rows expand the shared FINISH selector; they are not separate dispositions.

## Single Primary, Structurally

The presentation carries the primary as a single discriminated slot
(`none` / `resolve` / `clean_up` / `abandon`). Two glowing buttons are therefore
unrepresentable — there is no need for a runtime invariant scanning the bar for a second
primary. Whether a zone *renders* its verb (REVIEW always; RESOLVE when not stuck/continued;
FINISH per the table) is independent of which single slot glows.

## RESOLVE Zone: Address Feedback

Address feedback calls `POST /api/conversations/:id/pr-auto-fix-context`, captures the returned
message string, and posts it as a `UserMessage` to the conversation. It is enabled only when
`PrAutoFixAffordance.enabled = true` (`pr-association` `WorkActionsPrAffordanceContract`).

This verb is idle-only (REQ-WAB-005): posting a `UserMessage` to an errored or
context-exhausted conversation would be rejected or would silently reopen a non-resumable
state.

The feedback freshness label (`prFeedbackFreshnessLabel` from `ui/src/components/prBadge.ts`)
renders inline inside the Address feedback button when `PrStatusView.feedback_freshness` is
present (`"3 new"`, `"2 edited"`). The label is decorative — it tells the
user there is something new to address; it does not gate the button. The gate is the
addressability predicate of REQ-WAB-004: an open PR with the auto-fix affordance enabled,
regardless of check state or freshness.

When the PR's checks are confirmed passing, the honest `Merge PR #N ↗` link rides beside the
Address-feedback primary as a non-glowing secondary (`ResolveZone.secondary`), so a green PR
with review comments offers both "address the feedback" and "go merge it" at once. The
freshness and coverage markers ride on the primary verb only, never duplicated onto the
secondary link.

## RESOLVE Zone: PR Link Verbs

When `WorkDisposition = merge_ready`, the RESOLVE verb is an anchor tag (`<a>`) labelled
`"Merge PR #N ↗"`. When `WorkDisposition = pr_open_other`, it is labelled `"Open PR #N ↗"`.
The `address_feedback` secondary link renders the same `"Merge PR #N ↗"` anchor (just not
glowing). All point to `PrIdentity.url` with `target="_blank"` and `rel="noopener noreferrer"`,
where `N` is `PrIdentity.number`.

The label distinction is the honesty rule: "Merge" appears only when `check_state = passing`.
A pending, draft, or failing-with-affordance-disabled PR gets "Open PR" — the bar never
promises a merge the checks do not support. Phoenix has no merge API (REQ-WAB-010); the ↗
glyph signals external navigation, and neither verb calls a Phoenix endpoint.

## FINISH Zone: Clean Up

`Clean up` calls `POST /api/conversations/:id/mark-merged`. The git side effects are owned by
`work-lifecycle` REQ-WL-002: worktree deleted; task branch deleted for Work mode, kept for
Branch mode. No confirmation dialog.

## FINISH Zone: Abandon

`Abandon` calls `POST /api/conversations/:id/abandon-task` after a confirmation dialog.
`work-lifecycle` REQ-WL-001 owns the confirmation gate, the diff snapshot, and the git cleanup;
the bar's only responsibility is showing the dialog before the API call. The dialog copy is
mode-sensitive:

- **Work mode:** "Abandon this task? The worktree and task branch will be deleted."
- **Branch mode:** "Abandon this conversation? The worktree will be deleted but your branch
  will be kept."

## No Disabled-as-Status, No Toggle (REQ-WAB-008)

Every button in the bar is either enabled or absent. There are no disabled buttons left on
screen as status displays, and no click-to-enable toggles. The `gh_unavailable` disposition
renders an enabled, single-click `Clean up` primary with a warning note
("gh unavailable — manual cleanup"); cleanup proceeds without gh confirmation because gh cannot
confirm anything in that state and the user is the authority on whether the work is done.

## Inline Notes

The bar renders at most one inline note per render:

- **continued:** "Continued — actions belong on the continuation."
- **stuck with open/draft PR:** "PR #N still open — merge on GitHub, or abandon."
- **pr_closed:** "PR closed — abandon to clean up."
- **gh_unavailable:** "gh unavailable — manual cleanup."

## View Browser Exclusion (REQ-WAB-006)

The bar's REVIEW zone contains only `View Diff` (the diff viewer, `viewer_slot` REQ-VS-003 in
fullscreen presentation). The browser session affordance is not the bar's concern: the active
session inventory lives in the work scope (`work-scope-ui` REQ-WSUI-004) and is surfaced to the
user through the viewer slot's browser viewer on the session's rising edge (`viewer_slot`
REQ-VS-008). A browser session is the agent's artifact (the agent spawned it); reviewing it is
a different activity from reviewing the committed diff, and Phoenix has no "review the final
app" slot.

## Relationship to Sibling Specs

| Concern | Owner | What this bar does |
|---|---|---|
| PR identity, check state, feedback freshness | `pr-association` | Reads `PrStatusView`; never re-derives PR primary or the freshness/check signals |
| Auto-fix affordance + context message | `pr-association` (`WorkActionsPrAffordanceContract`, `WorkScopePrAutoFixContract`) | Reads the affordance; calls the context API; posts the returned message |
| Terminal action semantics | `work-lifecycle` | Calls mark-merged / abandon-task endpoints; shows Abandon's dialog |
| Action legality | bedrock `TaskResolved` | Server-enforced; bar does not re-gate |
| Diff viewer | `viewer_slot` (REQ-VS-003) | Opens the diff viewer in fullscreen |
| Browser session launcher | `work-scope-ui` (REQ-WSUI-004) + `viewer_slot` (REQ-VS-008) | Does not render; not the bar's concern |

## Design Decisions

**Single primary across all zones, encoded structurally.** The presentation carries one
primary-slot discriminator; a second glowing button cannot be represented. The bar answers
exactly one question per render: "what do I do next?"

**Disposition is a pure function, not component state.** `WorkDisposition` is re-derived on
every render from immutable inputs. There is no stored "bar mode" or toggle state, which makes
the bar predictable and testable: same inputs, same buttons.

**The disposition table is total over open PRs.** Every idle open-or-draft PR resolves to
exactly one of `address_feedback`, `merge_ready`, or `pr_open_other`; every stuck-with-PR case
resolves through the shared FINISH selector. There is no open-PR state that falls through to a
disabled or empty bar.

**Honest RESOLVE labels.** "Merge PR" appears only when checks are passing. A non-passing open
PR gets "Open PR", so the bar never offers a merge affordance the PR's checks do not support.

**RESOLVE suppressed in stuck phases, not disabled.** A disabled "Address feedback" on an
errored conversation would imply the user could resume by addressing feedback. Suppressing it
is the honest representation: there is no push-forward action when the conversation cannot
accept new messages.

**Merge opens GitHub, not a Phoenix endpoint.** Phoenix has no merge API and never merges or
pushes (`work-lifecycle` REQ-WL-002). The link verbs navigate to GitHub; the ↗ glyph signals
this.

**Tooltips on terminal verbs, not the RESOLVE zone.** The RESOLVE verbs are self-describing.
The terminal verbs share the same per-mode git effects; their real difference is Abandon's
diff snapshot and confirmation. Tooltips explain that difference so a first-time user does not
have to guess whether Abandon will destroy their branch.
