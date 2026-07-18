# Unify About diagnostics freshness and refresh behavior

Depends on the authoritative installation ownership task and should follow the identity/access restructuring.

## User outcome

A user can tell which `/about` data is live, when each snapshot was sampled, what refreshes automatically, and when the page is showing the last good value.

## Scope

- Replace the generic page `Refresh` action with explicit scope and coordinate deployment, disk, resource, and update refresh behavior without duplicate concurrent requests.
- Present deployment, disk, resources, and release discovery timestamps through one coherent freshness summary.
- Standardize loading, unavailable, stale-last-good, and hard-error language across sections.
- Preserve demand-driven resource sampling and bounded client history from the deployment-info contract.
- Avoid turning release discovery into high-frequency polling; manual release checks must remain explicit and backend status polling must remain cheap.
- Add timer/race tests for visibility changes, overlapping refreshes, stale recovery, and unmount cleanup.

## Acceptance criteria

- [ ] Every refresh control communicates its scope.
- [ ] Automatic versus manual refresh behavior is visible and predictable.
- [ ] Last-good values are labeled stale rather than disappearing.
- [ ] Release discovery is not repeated by transaction-status polling.
- [ ] No overlapping request or timer can overwrite a newer snapshot with an older one.
