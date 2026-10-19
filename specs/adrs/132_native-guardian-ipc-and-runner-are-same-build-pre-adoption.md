# ADR-132: Native guardian IPC and runner are same-build pre-adoption surfaces

- **Status:** Accepted
- **Date:** 2026-10-15
- **Affects:** REQ-SDC-011–020; `NativeRunnerBoundary`, `NativeGuardianIpcBoundary`, `NativeDispatchAdmissionBoundary`, `NativeGuardianPersistenceBoundary`, `NativeGuardianReconnectBoundary`, `NativeSameBuildBoundary`, `NativeGuardianQualificationBoundary`

## Context

The standalone controller has a qualified in-process policy loop, affine effect
dispatch, guardian custody, and a safe status/closeout CLI. Those tests cannot prove
that the native parent survives runner death or that a wire protocol preserves the
authority boundaries. ADR-131 selects detach-and-fence under controller loss and
requires the same living guardian for recovery; it does not select IPC framing or a
runnable executable surface.

Serializing an internal affine ticket would let ordinary deserialization manufacture
an authority that only successful persistence may issue. Conversely, adding native
run commands to the safe store CLI would expand a deliberately non-native surface.
The independent controller store also needs relational identity/session/inventory
and result bindings before reconnect can compare durable evidence with live custody.
These changes are pre-adoption work, not an obligation to migrate historical
controller stores or to integrate production installation.

## Options considered

1. **Extend the safe CLI and serialize internal tickets.** Reuses entry-point and
   type plumbing, but weakens both the safe-command boundary and affine issuance.
   A copyable JSON value is not a process-local capability.
2. **Separate same-build executables with bounded Unix IPC and guardian admission.**
   Keeps the CLI closed, gives launchd a surviving original parent, and makes wire
   data distinct from native authority. Costs explicit DTO conversion, live admission
   custody, normalized persistence, and real process-death tests.
3. **Build a version-negotiating supervisor service with durable replay and guardian
   replacement.** Could support broader upgrade scenarios but adds unsupported
   compatibility promises and cannot recreate original parenthood from database rows.
   Exactly-once execution cannot be inferred across native calls and SQLite commits.

## Decision

Choose option 2, refining rather than superseding ADR-131.

- Keep `phoenix-deployment-controller` status/closeout only. Provide separate runner
  and guardian executables built together. The runner owns the policy loop and store
  commits; the guardian owns original native child/group custody and retained results.
  A disposable launchd job owns the guardian, which directly parents harmless work.
- Use an owner-only Unix-domain stream socket in an owner-only directory, with local
  peer-owner checks. Use a four-byte big-endian length prefix and one typed UTF-8 JSON
  message, enforcing a shared finite build-defined payload limit before allocation.
  Exact build/protocol/attempt/live-instance handshake mismatches fail closed. This
  trusts code running as the owner; it is not a same-user adversarial sandbox.
- Define explicit wire DTOs rather than deserialize internal affine authority.
  The trusted runner consumes a committed ticket to request guardian admission.
  The guardian checks session, revision floor, recovery gate, and capacity, then mints
  an opaque one-use admission bound to exact live instance, session, attempt, key,
  and complete effect inputs. Dispatch consumes live registry authority before its
  single call. A replayed, stale, or wrong-bound token executes nothing. Reserve the
  whole mutation round's result capacity before the first mutation.
- On disconnect, fence the ended session and unused admissions without signals or
  new steps. Preserve children and results. Reconnect requires the same live guardian,
  exact original slot/native-identity inventory, unchanged original deadline, fresh
  durable interruption, and complete fresh reconciliation. A second connection does
  not silently replace an active owner; a replacement guardian does not recover one.
- Persist guardian identity, sessions, inventory, revision floors, and result
  application/ACK bindings in normalized controller rows. Rows remain evidence, not
  native authority. Send ACK only after the correlated policy application or explicit
  round settlement commits, naming the result and applying revision. Lost ACK is an
  evidence/acknowledgement retry, not permission to reapply or reexecute an effect.
- Limit protocol/schema support to same-build, admitted-version disposable execution.
  There is no schema-v1 upgrade, downgrade, rolling/mixed-version, live replacement,
  guardian-death/logout/reboot, or cross-boot deadline guarantee. This follows the
  explicit-compatibility policy in ADR-034, not a general migration framework.
- Require disposable native process-death/reconnect E2E, including commit/ACK crash
  windows and refusal paths. Keep the runnable loop separate from installer and
  `dev.py` orchestration. No production helper invocation, application DB mutation,
  artifact installation, activation, or production adoption is authorized. No
  exactly-once crash claim is made.

## Consequences

- **Positive:** Live native ownership has one original parent; runner restart can be
  qualified without silently expanding the safe CLI or production deployment seam.
  DTOs and one-use guardian admission preserve the distinction between wire data and
  authority. Normalized bindings make lost ACK and interrupted reconciliation visible.
- **Negative:** The native implementation must explicitly correlate DTOs with internal
  authority and retain bounded results. It needs additional schema rows and tests
  that actually kill processes under a disposable launchd job. A live guardian cannot
  be upgraded or replaced while retaining this guarantee; incompatible stores fail
  closed rather than receiving an automatic v1 migration.
- **Neutral:** Already-started work may continue after runner loss. Guardian loss and
  ambiguous native-call windows remain unresolved risk, not proof of cleanup or
  exactly-once execution. Installer integration and production adoption remain
  separately scoped; the native E2E is evidence for neither.

## References

- [Standalone controller requirements](../standalone-deployment-controller/requirements.md), REQ-SDC-011–020.
- [Standalone controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium), `DisposableNativeGuardianRunner` and its contracts.
- [Standalone controller status](../standalone-deployment-controller/executive.md).
- [ADR-034: Explicit compatibility guarantees](034_compatibility-guarantees-are-explicit-and-data-aware.md).
- [ADR-130: Controller durability and supervision](130_standalone-controller-owns-durability-and-supervision.md).
- [ADR-131: Detach-and-fence](131_controller-loss-detaches-and-fences-surviving-guardian.md).
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001 and REQ-COMP-005.
- [Production deployment requirements](../production-deployment/requirements.md) and [launchd deployment requirements](../launchd-deployment/requirements.md) retain their separate activation boundaries.
