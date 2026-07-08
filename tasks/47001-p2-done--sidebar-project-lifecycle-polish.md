# Sidebar project/lifecycle polish

## Goal

Make the sidebar easier to manage as conversation volume grows without adding a complex filtering system.

The product framing is:

- things I am actively using
- things that need my attention
- things that are done / cleaned up
- things I intentionally archived
- things by project

Project should be an orthogonal scope/cardinality split, not one more lifecycle bucket. The first pass should prioritize clarity, counts, and polish over new concepts.

## Current behavior to preserve / clarify

- The backend already exposes two list buckets: active/unarchived conversations and archived conversations.
- The current sidebar archive button is a mode switch, not an overlay: active and archived are mutually exclusive list modes.
- “Cleaned up” / terminal / completed conversations can remain in the active/unarchived list for now. Do not introduce a separate clean/done lifecycle state in this task.
- Existing state colors remain the primary source of truth for working / attention / error status. Do not add status filters in this task.
- Do not add search; command palette / cmd+p remains the search path for now.

## Proposed first pass

### 1. Replace confusing archive toggle copy with explicit lifecycle tabs

In the expanded sidebar, replace the single destination-labeled archive button with a clear segmented control, likely:

```text
Active  N    Archived  M
```

or similar concise labels.

Requirements:

- The current mode must be visually obvious.
- Counts must be visible for both active and archived within the current project scope.
- The control must not imply that archived is an additive toggle.
- Keep behavior equivalent to today: selecting Active shows unarchived conversations; selecting Archived shows archived conversations.

### 2. Make project scope orthogonal and count-aware

Keep project selection above the lifecycle control, but polish it as a scope selector rather than a flat, ambiguous pile of tabs.

Requirements:

- Show counts on `All` and each project.
- Counts should reflect active/unarchived conversations by default, unless a clearer dual-count presentation is chosen.
- Project selection continues to scope both active and archived views.
- Preserve the existing behavior that reveals the currently open conversation by clearing a project filter or switching archive mode when needed.

Possible simple UI direction:

```text
Projects
All 42
phoenix-ide 31
other-project 11

Active 31   Archived 120
```

When a project is selected, the lifecycle counts should reflect that project:

```text
Projects
All 42
phoenix-ide 31  [selected]
other-project 11

Active 31   Archived 53
```

### 3. Add lightweight list summary / empty-state clarity

When filters produce an empty list, make the reason clear:

- `No active conversations in phoenix-ide`
- `No archived conversations in phoenix-ide`
- `No active conversations`
- `No archived conversations`

Avoid introducing heavyweight explanatory text.

### 4. Preserve the expanded sidebar's visible window when collapsed

Collapsed mode should not suddenly expose many more conversations just because rows became dots. The user's mental/spatial model should remain stable: if the expanded sidebar effectively shows about eight or nine conversations in the current viewport, collapsing it should not turn that into twenty dots.

Requirements:

- Collapsed mode should limit visible conversation dots to roughly the same count/window a user had in expanded mode.
- The limit should compose with the same active/unarchived source list used today; do not add archived/project controls to collapsed mode in this pass.
- Prefer a simple, polished overflow affordance over showing every dot. Examples:
  - show the first N dots plus a `+M` overflow marker;
  - show only the currently visible/top N conversations;
  - preserve scroll position/count if straightforward.
- The active conversation should remain visible when possible, matching the existing sidebar reveal principle.
- Avoid making collapsed mode a second full navigation system; the goal is to reduce visual overwhelm while preserving orientation.

## Non-goals

- No search box.
- No new status filters such as Running / Attention / Error.
- No automatic archiving.
- No special treatment for clean worktrees or completed conversations.
- No backend schema change unless the existing API shape makes scoped counts impractical.
- No unarchive flow.

## Implementation notes

Likely UI touch points:

- `ui/src/components/Sidebar.tsx`
  - project scope state and project tab rendering
  - active/archived mode state
  - project-scoped counts
- `ui/src/components/ConversationList.tsx`
  - replace sidebar-only archive toggle with clearer lifecycle segmented control, or accept lifecycle controls from `Sidebar`
  - improve empty-state copy
- `ui/src/components/Sidebar.test.tsx`
  - update existing archive reveal tests
  - add coverage for scoped counts and empty states

The existing frontend receives both active and archived lists as props, so the first pass can probably compute counts client-side from the already-loaded lists.

## Acceptance criteria

- The expanded sidebar clearly communicates whether the user is viewing active or archived conversations.
- Active and archived counts are visible in the sidebar and update under project scope.
- Project selection remains orthogonal to active/archived selection.
- Selecting a project scopes both the active and archived lists.
- Opening a conversation that would be hidden by the current project/archive selection still reveals it, matching existing behavior.
- Empty states name the active project/lifecycle scope when applicable.
- Existing state-color indicators remain unchanged and remain the primary signal for working/attention/error state.
- Collapsed sidebar mode no longer shows a much larger number of conversations than expanded mode in the same viewport.
- Tests cover the revised archive/lifecycle control, project-scoped counts, reveal behavior, and collapsed-mode dot limiting/overflow behavior.
