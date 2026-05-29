# Make conversation chrome intentional: StatusBar for ambient truth, WorkControlBar for actions

## Problem

After PR #155, `StateBar` is the authoritative surface for working-phase visibility: server-authoritative phase entry time, first-byte/streaming distinction, heartbeat watchdog, retry suffixes, and connection-does-not-mask-agent-state behavior all live there. That is correct and should remain untouched.

Separately, Work/Branch conversations have GitHub PR status and lifecycle actions. Today those responsibilities are blurred:

- `StateBar` renders branch/PR metadata and also contains the actionful PR CI popover (`Auto-fix CI & address comments`).
- `WorkActions` independently fetches the same PR status and uses it for cleanup gating (`Clean up merged PR`, manual fallback, abandon).
- `WorkActions` mixes viewer launchers (`View Diff`, `View Browser`) with terminal lifecycle actions (`Mark as Merged`, `Abandon`).
- Context-exhausted Work/Branch lifecycle actions use bespoke rendering instead of sharing the same availability rules.

This creates duplicate PR polling, unclear names, and an unclear mental model: the bottom bar sometimes means runtime state, sometimes branch health, sometimes PR actions; the top strip sometimes means inspection tools and sometimes irreversible lifecycle decisions.

## Target design

Adopt two explicit surfaces:

1. **StatusBar = ambient truth**
   - Answers: “Where am I, and what is true right now?”
   - Owns conversation identity, mode, model, runtime/connection activity from PR #155, context usage, task/branch identity, and compact PR health summary.
   - Does **not** own irreversible Work/Branch actions.

2. **WorkControlBar = actionable work controls**
   - Answers: “What can I do next with this Work/Branch conversation?”
   - Owns viewer launchers, PR remediation actions, cleanup/mark-merged fallback, abandon, and continuation gating notices.

PR status is fetched once and shared between both surfaces.

## Normative spec map

- `specs/working-phase-visibility/` / `specs/llm-retry-visibility/`: StateBar remains the sole working-phase activity derivation surface. Do not rederive runtime/connection activity in Work controls.
- `REQ-CONV-005`, `REQ-CONV-007`: connection/activity indicators remain in StatusBar.
- `REQ-PROJ-011`: PR status remains visible in StateBar as the branch health indicator.
- `REQ-PROJ-026`, `REQ-PROJ-027`: Work/Branch mark-merged, cleanup, manual fallback, and abandon are WorkControlBar lifecycle actions.
- `REQ-BED-031`: continued parents disable terminal lifecycle actions; the live continuation owns the decision.
- Viewer slot specs / `REQ-BT-018`: viewer actions remain mutually exclusive with other viewer slots and should be separated from terminal lifecycle actions.

## Implementation plan

### 1. Introduce one PR status source

Create a shared hook or small provider near the conversation page boundary, e.g.:

```ts
useConversationPrStatus({ conversationId, convModeLabel, branchName })
```

It should:

- Fetch only for Work/Branch conversations with a branch.
- Clear stale status immediately on conversation/branch change.
- Refresh on visibility regain.
- Preserve the current 60s StateBar polling behavior if still desired for ambient PR status.
- Expose loading, unavailable, not-found, and found states explicitly enough that both surfaces can render without guessing.
- Reset any manual-cleanup fallback when a new PR status becomes available or the conversation/branch changes.

Do not let both `StateBar` and `WorkControlBar` call `api.getPrStatus` independently.

### 2. Split StateBar internally without changing its PR #155 runtime contract

Keep the exported component name `StateBar` initially to reduce churn, but extract named subcomponents/helpers so the file no longer reads as one mixed surface:

- `ConversationIdentityCluster` — slug, mode, model picker, project/task identity.
- `BranchPrSummary` — base/branch display plus compact PR badge / gh hint.
- `RuntimeActivityIndicator` or equivalent helper — the post-PR-155 state text/dot derivation.
- Existing `ContextIndicator` remains where it is.

Important: do not move or duplicate the working-phase derivation into Work controls. `phaseStateUpdatedAt`, `lastSseEventAt`, `firstByteRequestId`, and `turnRetryContext` stay StatusBar inputs.

### 3. Move actionful PR remediation out of StateBar

The compact PR badge in StateBar may still open a read-mostly detail popover with:

- PR title/link
- state/check summary
- feedback counts
- gh unavailable hints

But the action button `Auto-fix CI & address comments` should move to the Work control surface as a PR remediation action.

If retaining an action in the StateBar is necessary for UX, it must call into the same shared action model as WorkControlBar, not own separate state/fetching.

### 4. Rename and split WorkActions into WorkControlBar

Rename or wrap `WorkActions` as `WorkControlBar` and split internally:

- `WorkViewerActions`
  - `View Diff`
  - `View Browser` when browser session is active and not already open

- `PrRemediationActions`
  - `Address CI & comments` when PR is open and conversation input can send
  - link/open PR/checks if useful

- `WorkLifecycleActions`
  - `Clean up merged PR`
  - `Waiting for PR merge`
  - `PR closed without merge`
  - `Use manual fallback`
  - `Mark as Merged`
  - `Abandon`
  - continuation-disabled notice

The visible layout can remain compact; the important change is that these are named and tested as separate concepts.

### 5. Share lifecycle availability rules

Extract a pure derivation helper, e.g.:

```ts
deriveWorkLifecycleControls({
  convModeLabel,
  phaseType,
  continuedInConvId,
  prStatus,
  manualFallbackEnabled,
})
```

It should be used by normal idle WorkControlBar rendering and by context-exhausted Work/Branch terminal-action rendering where applicable.

Rules to preserve:

- Only Work/Branch conversations expose Work lifecycle controls.
- Normal WorkControlBar remains hidden while the agent is non-idle.
- Cleanup is disabled until PR status is known.
- Found but unmerged PR blocks cleanup unless explicit manual fallback is enabled and PR status is unavailable.
- Merged PR enables `Clean up merged PR` happy path.
- Closed-unmerged PR directs the user toward abandon.
- Continued parent disables abandon/mark-merged; actions belong on continuation.

### 6. Tests

Update/add tests for:

- One PR status fetch owner: StateBar + WorkControlBar rendered together cause one fetch cadence, not two independent calls.
- StateBar still renders compact PR badge and gh hints from shared status.
- WorkControlBar cleanup labels/gating:
  - loading → `Checking PR…`
  - merged → `Clean up merged PR`
  - open/draft → `Waiting for PR merge`
  - closed-unmerged → `PR closed without merge`
  - gh unavailable → `Use manual fallback`, then `Mark as Merged`
- `Address CI & comments` moved to/available from WorkControlBar and sends the same captured PR-context message.
- Continued parent disables lifecycle actions with the existing explanation.
- No branch / Direct / Explore conversations do not fetch PR status and do not render WorkControlBar.
- PR #155 StateBar behavior remains covered: elapsed/retry/heartbeat/first-byte tests should keep passing unchanged.

## Out of scope

- Backend PR association persistence (`tasks/67006-p1-ready--workscope-pr-association.md`) unless required for plumbing; this task should work with the existing `PrStatusResponse` endpoint.
- Changing the PR status backend or GitHub CLI behavior.
- Redesigning `convState`, `connectionState`, `ConversationAtom`, or PR #155 working-phase visibility semantics.
- Unifying the entire viewer-slot URL contract; only keep existing viewer actions working and better named.

## Acceptance criteria

- `StateBar` is clearly the ambient status/identity surface and still satisfies PR #155 working-phase visibility behavior.
- `WorkControlBar` is clearly the Work/Branch action surface and owns PR remediation + lifecycle actions.
- PR status is fetched from a single shared UI source for the active conversation/branch.
- StateBar continues to display PR status per `REQ-PROJ-011`.
- Work/Branch cleanup behavior remains PR-aware per `REQ-PROJ-026/027`.
- No duplicate GitHub CLI PR status polling from StateBar and WorkControlBar.
- Tests cover shared PR status, PR lifecycle gating, moved PR remediation action, and continued-parent gating.
- `./dev.py check` passes.
