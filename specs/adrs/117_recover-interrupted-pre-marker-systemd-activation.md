# ADR-117: Recover interrupted pre-marker systemd activation

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-COMP-014, REQ-PD-023–026; `ProtectedSystemdReleaseDeployment`

## Context

ADR-116 qualifies one exact systemd prefix65-to-RC4-target128 published-release transition. Its durable live migration marker is the irreversible boundary. Persistent service/socket inhibits prevent reboot or socket activation during disruption, but a helper or host interruption after `activating` and before that marker can strand the unchanged predecessor behind a claim and inhibits. The prepared-only activation/abandonment paths cannot safely infer what happened.

A stopped snapshot may not yet exist. The predecessor can legitimately write after preflight and during shutdown, so comparing recovery to a pre-stop logical digest would reject safe recovery. A completed stopped snapshot, by contrast, supplies the comparison authority for all later pre-marker recovery. Partial snapshot evidence is uncertainty, not permission to discard it.

The independently pinned candidate interprets an optional manual TLS CA before listener adoption. File existence alone does not prove certificate/key matching, CA capability, or chain trust. Similarly, discovering a missing exclusivity tool only after stopping production needlessly strands the transaction.

## Options considered

1. **Keep every pre-marker interruption manual-only.** Safe fencing remains but no bounded command can restore an otherwise proven predecessor.
2. **Resume activation or replay the backup.** Adds another candidate path and can conceal source changes or import generic rollback obligations.
3. **Admit explicit unchanged-predecessor restoration with retained evidence only.** Restores availability within the existing pre-marker boundary without database replay or a new compatibility interval.

## Decision

Choose option 3, scoped solely to ADR-116's protected systemd mode. `recover-systemd-current TXN` / retained helper `recover-current` selects by marker presence under the global operation lock and exact owned active claim.

Without a marker, require a validated retained manifest/controller/helper/rollback set, exact durable `activating` or `activation_failed_rollback_failed` status with `migration_started=false`, intact matching inhibits, and transaction-bound predecessor unit state. Capture/fsync that state before publishing `activating`. An interruption before state publication leaves the untouched `prepared` claim fenced and explicitly abandonable through the validated prepared path; missing state in a disrupted transaction never authorizes guessed recovery.

Stop both units, prove inactivity, resolve/probe `lsof`, and prove exclusive live database integrity/FK and the complete exact source-prefix65. Without snapshot evidence, permit normal predecessor writes before shutdown rather than comparing against a stale preflight digest. If either snapshot file or proof exists, require the complete valid snapshot/proof pair and exact live logical equality. Retain rehearsal evidence; never use it or a backup to replay the database. Recheck no marker, exclusivity and live evidence before restoration.

Restore and verify captured predecessor binary/configuration while inhibited. Remove only verified matching inhibits, reload/verify preserved configuration, restore and verify captured active/enabled state and applicable predecessor identity, then durably publish `activation_failed_rolled_back` and release only the exact claim. Any unproven or failed recovery retains ownership and inhibits; restoration failures attempt to reinstate inhibits and stop both units. Marker absence returns from this branch and never reaches candidate migration/start.

With any marker entry, including unsafe evidence, stop both fixed units and prove inactivity before detailed manifest decoding, dependency probing or evidence/DB validation. Retain ADR-116's exact-candidate-only forward recovery and prohibition on predecessor startup or snapshot replay. Missing dependencies fail stopped and fenced.

Activation resolves/probes exact usable `lsof` before inhibits or stop and uses that path for exclusivity. Staging and activation validate the preserved manual certificate/key through Python SSL, and any manual-only CA through service readability, single-certificate/capability validation and resolved/probed OpenSSL chain verification using only that CA and the supplied intermediates. No alternate TLS authority is persisted. Missing installed databases reject before handoff unless this is a genuine first install without predecessor evidence; ordinary protocol-v1 manifests stay unchanged.

This decision depends on ADR-116 and ADR-034's explicit compatibility policy. It does not change launchd, import its capacity policy, add startup auto-recovery, qualify another release/schema interval, authorize production execution, or introduce generic database rollback.

## Consequences

- **Positive:** Interrupted pre-marker availability can be recovered using exact retained evidence without weakening the live write boundary or rejecting normal pre-stop writes.
- **Negative:** Incomplete state/snapshot/inhibit evidence remains fenced and requires operator investigation. OpenSSL/Python SSL are conservative precondition checks, not proof that every certificate accepted by those libraries is accepted by the candidate's rustls/webpki implementation.
- **Neutral:** Source/fixture authorship and Python compilation do not establish executed regression coverage, real-systemd/reboot behavior, or deployment qualification. The pristine backup remains retained evidence after success.

## References

- [ADR-116](116_protect-exact-65-to-128-systemd-published-release-activation.md), exact systemd qualification and live boundary.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md), explicit compatibility policy.
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-014.
- [Production deployment requirements](../production-deployment/requirements.md), REQ-PD-023–026, and [behavior](../production-deployment/production-deployment.allium).
- `activate`, `_recover_pre_marker_locked`, `recover_current_path`, `validate_installed_tls`, and `preflight_database_exclusivity` in `scripts/systemd_deploy_helper.py`.
