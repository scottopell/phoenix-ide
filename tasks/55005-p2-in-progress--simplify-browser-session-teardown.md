# Simplify browser session teardown identity

## Objective

Remove redundant browser-session identity state and unnecessary linear scans without changing WorkScope ownership, restricted-session isolation, lifecycle behavior, or persisted browser profile paths. Also rename the WorkScope insertion helper so its name matches its post-migration responsibility.

## Evidence

`BrowserSessionManager::sessions` is keyed by the same synthesized session key copied into `ScopedSession::user_data_key`. During `spawn_kill_session_by_key`, teardown already receives that authoritative key but scans the map twice using `Arc::ptr_eq`: first to recover the duplicated profile key and later to recover the map key for removal.

The supplied key is sufficient to derive the browser user-data directory. Removal can address that key directly while retaining an identity check that the entry still refers to the session being terminated, protecting against removing a hypothetical replacement under the same key.

Separately, `Database::insert_work_scope_environment_tx` now inserts the complete authoritative `work_scopes` row—identity, authority, lifecycle, environment, and timestamps—rather than an environment projection row. Its old name is misleading after the merged schema simplification.

## Plan

1. Simplify browser teardown in `phoenix-browser`:
   - Remove `ScopedSession::user_data_key`.
   - Derive the user-data directory directly from the `key` passed to `spawn_kill_session_by_key`.
   - Replace both map-wide `Arc::ptr_eq` searches with direct keyed access/removal.
   - Before removal, verify the entry at that key still holds the same session `Arc`; do not remove a replacement entry.
   - Preserve concurrent-kill coordination, explicit Chrome termination, profile cleanup, lifecycle emission, and notification semantics.

2. Add or adjust focused tests proving:
   - Teardown removes only the requested session key.
   - Restricted actors with separate keys remain isolated.
   - A mismatched/replaced entry is not removed accidentally, if this race can be exercised without exposing test-only production APIs.
   - Existing kill idempotency and lifecycle behavior remain intact.

3. Rename `insert_work_scope_environment_tx` to a name reflecting that it inserts the complete WorkScope row, and update all call sites. Do not alter schema or SQL behavior.

4. Remove or update local comments that still describe the browser map as keyed only by `ResourceScopeKey::stable_key()`, since restricted-private sessions use a contained suffix. Keep comments factual and avoid expanding the unresolved design into code commentary.

## Explicit non-goals

- Do not change `session_key` formatting or restricted-private browser behavior.
- Do not introduce a typed browser-session key in this cleanup.
- Do not change `EffectiveResourceAccess`, `shared_restricted`, or authority rules.
- Do not delete or relocate `sub_agent_cwd_override`.
- Do not remove the read-only `work_scope_environments` view.
- Do not change `BrowserSessionManager::shutdown_all`; its explicit termination and lifecycle semantics require a separate correctness review.
- Do not modify the normative browser ownership requirements as part of this behavior-preserving cleanup.

## Validation

- Run the full `phoenix-browser` test suite.
- Run the full `phoenix-db` test suite.
- Run targeted browser kill/lifecycle/restricted-session tests with output visible when useful.
- Run `./dev.py check` (or the repository-approved affected lanes if gating selects them).
- Run `git diff --check`.

## Acceptance criteria

- `ScopedSession` no longer stores a duplicate profile/session key.
- Teardown performs no map-wide pointer scan to recover either the profile key or removal key.
- Direct keyed removal cannot remove an entry whose session identity differs from the session being terminated.
- Browser profile paths and restricted-session isolation remain byte-for-byte/behaviorally unchanged.
- The WorkScope insertion helper accurately names its complete-row write responsibility.
- Existing browser and DB tests pass, with focused regression coverage for the simplified teardown path.
