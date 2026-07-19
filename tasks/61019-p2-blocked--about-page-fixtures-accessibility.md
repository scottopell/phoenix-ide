# Add comprehensive About-page fixtures and accessibility coverage

Depends on the preceding About-page journey tasks.

## User outcome

The complete `/about` experience remains legible, keyboard-operable, and truthful across desktop/mobile, local/remote, healthy/stale, and update/recovery states.

## Scope

- Add a dedicated Ladle fixture with scenario data for managed backends, development/unmanaged/ambiguous ownership, local/remote access, resource staleness, disk cleanup states, update availability, active handoff, rollback, and recovery failure.
- Add `dev.py qa` wiring and deterministic capture scripts following Phoenix fixture conventions.
- Audit heading hierarchy, live-region usage, button labels, focus return after confirmation, table semantics, status color contrast, and narrow-screen overflow.
- Add journey-level component tests that exercise the integrated page rather than only isolated panels.
- Update deployment-info executive verification coverage after captures and tests exist.

## Acceptance criteria

- [ ] Representative `/about` states can be reviewed without a live production installation.
- [ ] All actions are keyboard accessible with deterministic focus behavior.
- [ ] Status meaning is not conveyed by color alone.
- [ ] Long paths, commits, checksums, and failure messages remain usable on narrow screens.
- [ ] QA captures cover local/remote and success/recovery journeys.
