# Native macOS launchd Deployment — Executive Status

## Current reality

The protocol-2 helper implements one ordinary local/published deployment path.
It validates exact identity, hash, complete plist/environment and atomic staging
before disruption; quarantines the canonical plist; confirms predecessor stop;
installs the candidate; durably records `migration_started`; and runs exact
candidate `--migrate-only` once with a 600-second bound and inherited log output.
It boots the private plist, verifies stable new PID plus exact version/full SHA,
publishes that same inode, writes deployed SHA and committed status, and releases
its claim. Durable committed is terminal for runtime behavior; claim-release,
reporting and cleanup errors preserve the healthy runtime and committed status.
An incomplete release retains temporary binary copies and any surviving marker
for operator inspection. Warning output is best effort.

After disruption failure before durable committed it attempts stop and retains
`failed`, a single phase/error/stop-failure diagnostic, the claim and transaction evidence. There is
no rollback, resume, repair, finalization or replay. All active deploy/restart
markers fence a new operation regardless of terminal status. Historical on-disk
transactions remain untouched; the new helper refuses old manifests.

Installed-runtime restart remains a separate independent helper that preserves
binary, plist, environment, listener and deployed SHA and verifies the same exact
identity on a new PID. ADR-229 records the accepted availability/manual-recovery
cost and supersedes the retired macOS recovery routes.

## Requirement coverage

| Requirements | Current implementation / verification |
| --- | --- |
| REQ-LDD-001–007,009–012,029 | `launchd_prod_deploy`, helper `validate_pre_disruption_handoff`/`activate`; ordinary source/identity/configuration/concurrency and real candidate migration tests |
| REQ-LDD-013 | Disposable native launchd harness rejects production resources |
| REQ-LDD-014–016 | Installed-state restart helper; 38 focused tests and real initiator-death journey |
| REQ-LDD-030–031 | Failed status/retained marker/evidence, explicit stop uncertainty, old-manifest refusal, UI/API manual-recovery block |
| REQ-LDD-032 | Claim-release, reporting and cleanup failures preserve committed status and the healthy runtime; warning output is best effort |
| REQ-LDD-008,017–028 | Withdrawn by ADR-229; no active guarantee |

## Verification and deliberate test retirement

Reported focused results: helper 25; installer 60 (53 ordinary retained plus
seven new manual-recovery/isolation tests); restart 38; harness safety five; and
ReleaseUpdatePanel 32 pass. Real disposable launchd healthy commit, partial-write
migration failure, health failure and restart after initiator SIGKILL pass. Helper
tests exercise actual SQLite candidate subprocess writes without parent database
reads, durable marker ordering, partial-write retention/no predecessor, health/PID
failure, stop failure, protocol refusal, same-inode publication, cleanup failure,
post-commit output failure, claim-release failure and unlink-then-fsync failure.

The 6,406-line `test_current_schema_prepared_upgrade.py` and old manifest,
Activation, schema114/123/128/129, repair, refresh and rollback classes are retired
with their withdrawn contracts. `test_launchd_deploy.py` shrinks from 7,357 to
1,257 lines. Their identity/source/configuration/concurrency and essential failure
coverage is carried by the helper, ordinary installer and disposable harness;
obsolete recovery promises are intentionally not reproduced. The unused Rust
controller and its prepared validation harness are retired separately, not adopted.
The Rust `current_schema_controller_accepts_compiled_123_and_128_shapes` and its
single-use embedded Python hook are retired with deployment-side schema admission;
migration128 data/constraint and migration-lineage tests remain. The maintained
macOS app workflow runs ordinary helper, installer, harness-safety and restart
tests instead of the removed prepared-upgrade module.

Full TypeScript checking is blocked by older primary dependencies missing patched
micromark/mdast modules; no wire type shape changed. No production deployment was
performed; production remains `9c5d471d`. Focused and disposable results do not
claim hosted/full-repository or production acceptance.

## Operator recovery

Inspect logs, retained evidence and actual service/helper state; preserve the
failed transaction. Confirm exact helper absence and resolve target runtime state
before explicitly clearing the matching marker and deploying a chosen forward
candidate. An older binary requires its matching offline database restore with
all users stopped. See `LAUNCHD.md`; historical helper commands are not recovery APIs.
