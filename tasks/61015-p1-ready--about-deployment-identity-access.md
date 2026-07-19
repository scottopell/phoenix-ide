# Unify About deployment identity and access context

Depends on the authoritative installation ownership task.

## User outcome

A user opening `/about` can answer, in one glance: what Phoenix build is running, how it is managed, which host it is on, and whether this browser can perform host-local actions.

## Scope

- Restructure the top of `/about` around one runtime identity/ownership summary instead of repeating version and SHA in Build and Phoenix updates.
- Show installation ownership with truthful labels for managed, development, unmanaged, ambiguous, and unsupported states.
- Add one visible `Viewing locally` / `Viewing remotely` access indicator and one consistent explanation of the server-host boundary.
- Keep ownership, browser locality, and update eligibility visually distinct.
- Remove redundant current-version presentation from the update panel while preserving candidate-vs-running comparison.
- Replace history-dependent or vague navigation wording where a deterministic destination is available.
- Update deployment-info/release-update requirements and component tests for the new information hierarchy.

## Acceptance criteria

- [ ] Running version/SHA has one primary presentation.
- [ ] Management technique is shown only when proven by the shared model.
- [ ] Remote users understand why host-local reveal and approval actions are unavailable before looking for a missing button.
- [ ] Development and unmanaged instances receive useful, non-alarming language.
- [ ] Mobile and desktop layouts retain high information density without duplicate facts.
