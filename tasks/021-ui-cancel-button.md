---
created: 2026-01-31
priority: p2
status: ready
---

# Add Cancel Button to Web UI

## Summary

Add a cancel button to the web UI that allows users to abort in-progress operations.

## Context

The backend already supports cancellation via `POST /api/conversations/:id/cancel` (REQ-API-004). The UI currently has no way to trigger this - users must wait for operations to complete.

## Acceptance Criteria

- [ ] Cancel button appears when agent is working
- [ ] Cancel button replaces or appears next to Send button
- [ ] Clicking cancel calls the cancel API endpoint
- [ ] UI shows "cancelling..." state while cancellation is in progress
- [ ] Button is disabled during cancellation to prevent double-clicks
- [ ] Touch-friendly size (min 44x44px)

## Notes

- See `static/app.js` `state.agentWorking` for when to show
- Cancel API is already implemented in `src/api/handlers.rs`
- Consider red/destructive button styling
