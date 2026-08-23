# Increase launchd deployment readiness budget

Increase only the default exact-identity health/readiness verification budget for launchd production activation and rollback. Keep exact expected version/full-SHA verification, the separate process-transition timeout, and existing activation/rollback semantics unchanged.

## Failure model

Under observed host load, launchd process transitions and listener acquisition completed within the existing 30-second transition timeout. Candidate verification crossed the separate 30-second exact-identity health budget at 31.35 seconds, and rollback HTTPS exact identity became available at 35.78 seconds, after the same health budget had already reported rollback failure.

## Acceptance evidence

- Use a conservative bounded exact-identity readiness budget justified by observed startup under load.
- Improve timeout diagnostics with elapsed time, deadline, and last observation when local to the verification helper.
- Deterministic fake-clock tests prove readiness after 30 seconds but within the new budget succeeds, true budget expiry fails, and exact identity mismatch never succeeds.
- Run launchd helper and relevant dev.py deployment tests; do not deploy or manipulate production.

This follow-up is independent of and does not block ProductConversation / PR #710. It does not change compatibility policy, rollback semantics, deployment transaction states, database handling, startup activation gating, or task 44016.
