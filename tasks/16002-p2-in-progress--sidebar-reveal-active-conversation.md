# Ensure notification-opened conversations are visible in the sidebar

## Problem

When a user clicks a Phoenix desktop notification, Phoenix navigates to the target conversation, but the sidebar does not always make that conversation easy to find. The active row may be off-screen, hidden inside a collapsed chain, or excluded by a persisted project filter.

## Goal

After any route navigation to `/c/:slug` — including notification clicks, direct links, command palette navigation, or normal row clicks — the expanded sidebar should make the active conversation visible in the side panel.

## Scope

- Add active-conversation sidebar visibility behavior in the UI.
- When `activeSlug` changes in the sidebar conversation list:
  - ensure the active row is scrolled into view with `scrollIntoView({ block: 'nearest', behavior: 'smooth' })` or equivalent;
  - if the active conversation belongs to a chain that is currently collapsed, expand that chain so the active member row is mounted and visible;
  - avoid disruptive scrolling when the active row is already visible.
- In `Sidebar`, if the current project filter hides the active conversation, clear the project filter so the active conversation can appear.
- Preserve existing user behavior for manual chain collapse/expand except where necessary to reveal the active conversation.
- Add or update tests covering:
  - active standalone conversation scrolls into view;
  - active chain member expands its chain and scrolls into view;
  - project filter is cleared when it hides the active conversation;
  - no regressions to existing chain grouping and active-row rendering.

## Notes

This is intentionally a UI-side improvement. It does not require service-worker notification routing because the desired behavior applies to any navigation that changes the active conversation, not just notification clicks.
