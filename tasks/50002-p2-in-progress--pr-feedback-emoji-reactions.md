# Add emoji reaction status indicators to PR feedback awareness

## Problem

GitHub PR feedback can carry important workflow state through emoji reactions. In particular, reviewers and bots use reactions such as:

- `eyes` / 👀 — review or investigation is in progress / acknowledged.
- `+1` / 👍 — feedback is approved, accepted, or considered good.

Phoenix currently treats PR feedback primarily as comment text plus thread resolution. The `Address feedback` / PR feedback pipeline does not expose reaction-derived status, so UI surfaces such as the sidebar cannot show whether feedback is merely open, already being looked at, or effectively approved. This makes the sidebar harder to skim and forces users to hover/open GitHub to understand feedback state.

## Goal

Model emoji reactions as typed PR feedback status signals and make them available to UI consumers, especially sidebar/work-summary surfaces, so Phoenix can show richer feedback state at a glance.

## Scope

Add reaction-aware PR feedback status in the existing PR feedback/status pipeline:

- Capture relevant GitHub reactions for PR feedback items.
- Preserve raw typed reaction metadata where useful for debugging/future UI.
- Derive a compact feedback status from reactions, at minimum:
  - `in_progress` when an actionable feedback item has an `eyes` reaction.
  - `approved` when an actionable feedback item has a `+1` / thumbs-up reaction.
  - `open` / default when no status reaction is present.
- Thread the derived status through backend API types used by PR status/sidebar consumers.
- Render the status in the sidebar or existing PR feedback summary UI using compact emoji/symbol indicators consistent with Phoenix’s information-dense UI philosophy.
- Keep the `Address feedback` artifact/prompt aware of reactions as context, but do not make the artifact the only consumer.
- Add focused tests for capture, status derivation, serialization/API shape, and UI rendering/labels where applicable.

## Proposed design

### Backend model

Add typed reaction data, for example:

```rust
pub struct PrFeedbackReaction {
    pub author: String,
    pub content: String,
    pub created_at: Option<String>,
}
```

Add a derived status enum, for example:

```rust
pub enum PrFeedbackStatus {
    Open,
    InProgress,
    Approved,
}
```

Attach the status either per `PrFeedbackItem` or as an aggregate summary field, depending on the existing UI data flow. Prefer correct-by-construction typing over stringly-typed status labels.

If multiple status reactions are present, define and test precedence. Suggested initial precedence for skim UI:

1. `approved` if any actionable item has a thumbs-up approval signal.
2. `in_progress` if any actionable item has an eyes signal and no approval signal wins.
3. `open` otherwise.

If per-item display is more useful than aggregate display, expose both the item-level status and an aggregate summary count.

### GitHub capture

Extend `crates/phoenix-ide/src/api/pr_monitoring.rs` GitHub fetches to include reactions for issue comments, review comments, review summaries, and review-thread comments where supported.

Implementation notes:

- GraphQL review-thread comments can query reactions/reaction groups; use a bounded list or count by reaction content.
- REST comment endpoints may require the GitHub reactions media type or separate bounded reaction endpoints.
- If reaction details cannot be fetched for a surface, log and surface coverage honestly rather than silently treating missing reaction data as “no status.”

### Concrete UI

Build the first visible UI in the existing conversation sidebar PR badge:

- Target component: `ui/src/components/ConversationList.tsx`, specifically `SidebarPrBadge` / `PrBadge`, which currently renders compact labels like `#375`, `#375 draft`, `#375 merged` for each conversation row.
- Extend the cached PR summary shape that feeds `ConversationList` so each row can carry the derived reaction status for its associated PR feedback.
- Render the reaction status inline inside the existing `.sidebar-pr-badge` after the PR number/state text:
  - Open/no reaction status: unchanged, e.g. `#375`.
  - In progress: `#375 👀`.
  - Approved: `#375 👍`.
- Update the badge tooltip (`sidebarPrTooltip`) to include a text explanation:
  - `Feedback status: in progress (eyes reaction)`.
  - `Feedback status: approved (thumbs-up reaction)`.
- Update `aria-label` for both non-interactive `<span>` and interactive `<a>` badges so screen readers get the same status text, e.g. `PR 375, feedback in progress`.
- Add status-specific CSS hooks on the same badge element, for example `sidebar-pr-badge--feedback-in-progress` and `sidebar-pr-badge--feedback-approved`, so future styling can color/tune the indicators without DOM churn. Keep the initial styling compact; do not add a second row or verbose text to the sidebar.
- Add/extend `ui/src/components/ConversationList.test.tsx` coverage:
  - no status reaction keeps the existing `#N` rendering;
  - `in_progress` renders `#N 👀`, has an explanatory tooltip, and has accessible text;
  - `approved` renders `#N 👍`, has an explanatory tooltip, and has accessible text;
  - mobile/non-interactive badge rendering preserves the same text and aria behavior without becoming a link.

Preserve existing freshness/new/edited signals elsewhere (`WorkActions.tsx`, `prFeedbackFreshnessLabel`); sidebar reaction status is a skim indicator, not a replacement for freshness.

### Address-feedback context

The PR auto-fix context artifact should include reactions/status so an agent addressing feedback can see reviewer intent. Update the instruction text to explain that reactions are status/context signals, not proof that a comment has been resolved unless GitHub thread resolution says so.

### Freshness semantics

Treat reaction changes deliberately:

- A new reaction on an existing actionable feedback item should not create a new feedback identity.
- It may count as an edited/status-changed item for freshness if that helps the UI indicate status changed since the last capture.
- Resolved threads remain non-actionable regardless of reactions.

## Verification

Suggested checks:

```bash
./dev.py check
```

Plus targeted backend/UI tests for:

- `eyes` reaction derives `in_progress`.
- `+1` / 👍 reaction derives `approved`.
- Reaction status survives dedupe/merge between REST and GraphQL feedback sources.
- API serialization includes the typed status expected by UI consumers.
- Sidebar/work-summary rendering shows the compact indicator with accessible text.

## Out of scope

- Automatically resolving threads based on reactions.
- Treating reactions as branch-health or lifecycle gates.
- Mirroring all GitHub notification/activity events.
- Building a full reaction management UI for adding/removing reactions.
