# ADR-229: Simplify macOS deployment to candidate migration and manual recovery

- **Status:** Accepted
- **Date:** 2026-11-06
- **Affects:** REQ-LDD-001–032, REQ-PD-002–012, REQ-PD-018–022, REQ-PD-028, REQ-PD-030–031, REQ-COMP-001, REQ-COMP-003, REQ-COMP-012–013, REQ-COMP-015, REQ-COMP-017–019, REQ-COMP-021, REQ-SDC-001–023, REQ-RU-008

## Context

The macOS helper and installer have accumulated protected prepared upgrades,
registry-specific admission, snapshots, rehearsals, paired restoration, finalization,
forward resumes, sealed repairs, and interrupted-transaction classifiers. The
standalone Rust controller adds durable custody, native IPC and resource policy
without a production caller. Agent-authored requirements and their regression
suites perpetuate these mechanisms without demonstrating corresponding product
need.

The operator explicitly chooses the simpler failure contract: “The candidate may
have changed the database. I stopped. Here is what ran and failed; recovery
requires an operator.” Asked to adopt that tradeoff, the user says: “This is my
desired tradeoff, adopt my old philosophy and aggressively delete and simplify
please.” This accepts service downtime and manual recovery instead of preserving
historical automatic compatibility guarantees.

## Options considered

1. Retain the schema-specific Python paths and their recovery APIs. Preserves
   historic automation but retains most of the complexity and qualification cost.
2. Adopt the standalone Rust deployment controller. Adds a policy/store/native
   guardian architecture before there is a production integration need.
3. Use one ordinary macOS path: stage and validate artifacts, stop, run the exact
   candidate's migrations once, start, verify and publish; stop and retain evidence
   on failure. Database recovery belongs to the operator.

## Decision

Choose option 3. Preserve independent launchd ownership, immutable source and
checksum pins, exact candidate identity, atomic install/publication, bounded
launchd observation, mutual exclusion, durable diagnostics and installed-state
restart. A protocol-2 helper accepts only `local_head` and `published_release`.
It validates the candidate and complete plist before disruption, quarantines the
canonical plist, confirms predecessor stop, installs the binary and persists
`migration_started` before invoking exact candidate `--migrate-only` once with a
600-second limit. Output goes directly to the activation log. The candidate owns
migration eligibility and reconciliation; routine deployment does not open the
production database, parse registries, scan integrity/foreign keys/logical contents,
snapshot or rehearse.

After private-plist startup, stable new PID and exact version/full-SHA verification
permit same-inode canonical plist publication, deployed identity and durable commit,
then an owned claim-release attempt. Durable committed is terminal for runtime
behavior: claim-release, reporting and cleanup errors must not stop the verified
runtime or change committed status. Failed release leaves any surviving marker
for manual inspection and retains temporary binary copies; it does not recreate
an already removed marker. Warnings are best effort even if the log sink fails.
Temporary candidate/previous binary cleanup follows completed claim release.
Preparation errors leave the service intact and release only the transaction's
own marker.

Any post-disruption error before durable committed attempts stop, removes
attempted candidate publication, records `failed` with the phase/error and any
stop uncertainty, and retains the
claim and artifacts. Publication errors before durable committed take this path.
No automatic predecessor startup, database restore, resume, finalization or replay
exists. Every active deploy/restart marker blocks a new operation until its owner
releases it or an operator resolves it explicitly; terminal status is not a stale
marker reclamation rule. The UI/API recognize failed deployment and block another
in-app approval pending recovery.

Manual recovery means inspecting logs, retained transaction evidence and actual
service/helper state; preserving that evidence; confirming the exact helper is
absent and resolving the target runtime; explicitly clearing only the matching
marker; and deploying a chosen forward candidate as a new transaction. An older
binary requires a matching database backup restored offline with all database
users stopped. Previous runtime artifacts do not establish compatibility.
Historical transaction directories, manifests and retained helper bytes remain
untouched. The new helper refuses protocol 1; it neither converts nor replays
historical evidence.

Withdraw REQ-LDD-008 and REQ-LDD-017–028; REQ-PD-018–020,022,028,030;
REQ-COMP-012,013,015,017,018,019; and REQ-SDC-001–023. Keep concise tombstones so
IDs cannot be reused. Preserve the Linux portion of REQ-PD-021 and all protected
Linux contracts REQ-PD-023–027/029 and REQ-COMP-014/016. Ordinary Linux
runtime-artifact rollback is unchanged. This does not withdraw migration-owned
lineage validation in the candidate or other unrelated compatibility requirements.

This decision supersedes the macOS rollback/recovery and schema-specific portions
of ADR-010,102,103,105,106,114,115,125,126,128,148,190, and retires the standalone
controller decisions ADR-133–141. ADR-010's independent helper principle remains.
The exact Linux artifact/preparation decisions ADR-129 and ADR-130 remain active;
ADR-131 concerns conversation naming and is unaffected. All accepted historical
ADR text remains unchanged. Historical decisions explain old evidence; they are
not active operator instructions.

Remove the unused controller crate and its dedicated workflow, artifact fixture
and prepared validation harness rather than adopting another framework. Additional
generalization, compatibility or recovery machinery needs demonstrated product
need. Agent-authored requirements alone are not evidence of that need; this is a
design constraint, not a new approval workflow.

## Consequences

- **Positive:** one ordinary macOS lifecycle; candidate migrations remain the
  database authority; substantially less deployment code, policy and test upkeep.
- **Negative:** failed or interrupted migration may leave partial committed data
  and an unavailable service. Stop can fail. No automated restoration, forward
  resumption, finalization or prior transaction compatibility is promised. An
  operator must diagnose and recover; downgrade needs a matching offline backup.
- **Neutral:** installed-runtime restart and Linux activation/recovery retain their
  existing contracts.

Test retirement is deliberate contract retirement, not deletion to make checks
green. `test_current_schema_prepared_upgrade.py` (6,406 lines) and the manifest,
Activation, protected schema114/123/128/129, repair, refresh and rollback test
classes disappear with their withdrawn routes. `test_launchd_deploy.py` shrinks
from 7,357 to 1,257 lines and retains 53 ordinary installer tests plus seven new
manual-recovery/isolation tests. Essential failure, exact identity, source,
configuration and concurrency coverage remains in 25 small-helper tests, 60
installer tests and disposable launchd journeys. The removed controller's 54
files include its own retired tests; no production caller or behavior is lost.
The prepared validation harness disappears with that unused controller, while the
ordinary disposable launchd harness remains.

The Rust `current_schema_controller_accepts_compiled_123_and_128_shapes` test and
its single-use `validate_current_schema_with_controller` Python hook are also
retired: they assert the withdrawn deployment-side schema123/128 admission
contract. Actual migration128 data/constraint tests and migration lineage tests
remain. The maintained macOS app workflow runs the replacement ordinary helper,
installer, harness-safety and restart suites instead of the removed prepared
upgrade module; ordinary macOS qualification remains in place.

## References

- `specs/launchd-deployment/requirements.md` and `launchd-deployment.allium`
- `specs/production-deployment/requirements.md` and `production-deployment.allium`
- `specs/compatibility/requirements.md`
- `specs/standalone-deployment-controller/requirements.md` (withdrawn identifiers)
- `scripts/launchd_deploy_helper.py`: `activate`, `validate_pre_disruption_handoff`
- `dev.py`: `launchd_prod_deploy`, `_validate_launchd_handoff`
- `LAUNCHD.md` and `.agents/skills/phoenix-deployment/LAUNCHD.md`
