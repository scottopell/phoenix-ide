# Standalone Deployment Controller — Executive Status

## Current reality

A new std-only Rust policy crate models deployment phases, resource admission,
stable process identity, owned-tree evidence, bounded cleanup, interruption,
administrative closeout, and the post-migration forward-recovery fence. The crate
has no production adapter or caller. Existing deployment behavior remains
unchanged.

The deterministic fake-OS suite covers the observed child-ESRCH-before-birth race,
PID reuse, reparenting, exact-identity signaling, permission uncertainty, absolute
deadline closeout, interruption reconciliation, sustained contention, and exact
candidate forward recovery. The Allium model validates locally. Rust compilation,
tests, and Clippy require hosted CI because the local host did not satisfy its
compiler-admission capacity threshold.

Native macOS adapter work and harmless isolated-label integration tests remain
future adoption slices. No production launchd label, process, reservation, or
installation path is used by this crate.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-SDC-001 | `Transition` separates the durable checkpoint from `after_persist` OS effects; interruption tests exercise recovery from the checkpoint. No durable store adapter exists yet. |
| REQ-SDC-002 | `ResourceWindow` uses ordered capacity/contention samples with sustained-contention regression coverage and contains no process metadata. |
| REQ-SDC-003 | `ProcessIdentity`, `BirthToken`, `DiscoveryId`, and `CandidateDiscovery` separate stable ownership from PID hints. |
| REQ-SDC-004 | Typed observation failures remain unresolved through the deadline; the 5188/5189 fake-OS regression covers transient ESRCH before birth acquisition. |
| REQ-SDC-005 | `OwnedProcessTree` and the fake OS recheck exact identity before signaling; PID reuse and reparenting regressions are present. |
| REQ-SDC-006 | `CleanupReadiness::VerifiedClean` requires all exact identities absent and no unresolved discovery. |
| REQ-SDC-007 | Attempt-bound `AdministrativeRiskAcceptance` closes without a cleanup claim or OS effect and retains evidence. |
| REQ-SDC-008 | Recovery marks owned identities for fresh observation and preserves the original deadline before exposing signal effects. |
| REQ-SDC-009 | `CompatibilityBoundary::CandidateMigrationInvoked` retains full commit plus artifact digest and recovers only through candidate reverification. |
| REQ-SDC-010 | `ControllerOs` is typed and fake-backed. Native macOS qualification and production adoption are not implemented. |

## Next work

Qualify the exact source commit through the existing hosted Rust, Clippy, and spec
checks. After the policy core is green, add a separately reviewed macOS adapter and
harmless isolated integration fixture. Durable storage and thin `dev.py` delegation
must be serialized with the existing installer and host reconciliation owners;
production adoption remains opt-in until those slices are reviewed.
