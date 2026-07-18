# Polish release updates within the full About journey

Depends on the authoritative installation ownership and identity/access tasks.

## User outcome

Official GitHub release updates feel like a native part of `/about`: availability, authority, preparation, reconnect, rollback, and recovery all read coherently against the proven installation technique.

## Scope

- Derive supported backend and approval eligibility from the shared ownership model; distinguish ownership from missing tools, privilege, locality, and non-production policy.
- Present `latest stable`, `update available`, discovery unavailable, blocked, active, committed, verified rollback, rollback failure, unreadable, and stale states with concise next actions.
- Keep immutable tag/commit/asset/checksum review and explicit confirmation, while removing duplicated running identity.
- Make reconnect expectations and backend ownership visible without exposing controller implementation detail.
- Ensure unsupported/unmanaged/ambiguous installations receive repair or operator guidance rather than an install button.
- Review approved-marker/controller artifact retention and cleanup so the audit trail is durable but bounded.
- Extend release-update specs and tests for every ownership and authority combination.

## Acceptance criteria

- [ ] Only officially published stable GitHub releases are offered.
- [ ] An update button appears only for a proven supported production owner and a newer immutable preview.
- [ ] Remote, unmanaged, ambiguous, unsupported, and missing-prerequisite states explain what blocks approval.
- [ ] Reconnect restores the backend-owned transaction outcome without a parallel app status copy.
- [ ] Rollback success and rollback failure remain visually and semantically distinct.
