---
created: 2026-04-08
priority: p2
status: ready
artifact: ui/src/components/TaskApprovalReader.tsx
---

# Noticeable lag after clicking approve on task proposal

## Problem

When clicking the green "Approve" button on a task proposal in an Explore
conversation, there's a ~5 second delay before anything changes visually.
The button click feels unresponsive -- no loading state, no spinner, no
"approving..." text. The user doesn't know if the click registered.

The delay is real work: the backend creates a worktree, writes the task
file, commits, and transitions the conversation. But the frontend gives
no feedback during this time.

## What to fix

1. **Immediate visual feedback on click**: The approve button should show
   a loading/spinner state immediately ("Approving..." or a spinner icon).
   Disable the button to prevent double-clicks.

2. **Optimistic state transition**: Consider transitioning the UI out of
   the approval overlay immediately (with a "Setting up worktree..." 
   message) rather than waiting for the full SSE state change.

3. **Investigate the 5s delay**: Profile where the time is spent. Is it
   the git worktree creation? The LLM request that fires after approval?
   The SSE round-trip? If the worktree creation is slow, the system
   message ("Task approved. You are on branch...") should arrive faster
   than the LLM response and could trigger the UI transition.

## Done when

- [ ] Approve button shows loading state immediately on click
- [ ] User has visual feedback within 200ms of clicking approve
- [ ] Button is disabled during approval to prevent double-clicks
