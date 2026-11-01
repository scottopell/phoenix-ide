# ADR-141: Disposable launchd validation precedes installer adoption

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-SDC-023; exact PreparedSource114 launchd installer validation
- **Extends:** [ADR-125](125_qualify-exact-prepared-source114-candidate.md), [ADR-133](133_standalone-controller-owns-durability-and-supervision.md), [ADR-135](135_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md)

## Context

The standalone controller reaches `ReadyForDisruption` in a real disposable launchd guardian/runner journey, but it has no production installer caller. The production launchd installer owns a separately qualified transaction: candidate and controller-source binding, claim and ledger policy, application-database protection, service disruption, publication, health verification, and rollback or forward recovery.

Calling the production deploy command with path overrides would not form a safe integration test. It acquires production-shaped authority before candidate preparation and can reach irreversible operations. Conversely, testing only copied manifest dictionaries would not prove that the real installer and retained helper agree on their closed typed contract.

## Options considered

1. **Integrate the controller into the production launchd installer immediately.** This crosses
   claim, service-control, database-migration, publication, and recovery boundaries before the
   exact handoff is qualified.
2. **Validate a synthetic protocol or fake executable fixture.** This cannot prove compatibility
   with the retained helper, signed candidate, immutable migration registry, or real manifest
   loader and classifier.
3. **Qualify the real pre-disruption handoff under a generated disposable root.** Exercise the
   production admission and read-only validation seams while withholding every production
   mutation capability.

## Decision

Choose option 3.

A validation-only boundary SHALL exercise the real installer-to-helper handoff before production adoption.

The boundary SHALL:

1. generate its own owner-private temporary root and admit the exact PreparedSource114 artifact through the production sequence: receipt/schema and candidate-byte digest, Developer ID signature, hardened runtime, notarization, stapling, and Gatekeeper before execution, followed by exact build-identity commit/version verification;
2. derive the inert predecessor witness, disposable service label, transaction path, target path, evidence path, and health endpoint internally rather than accepting executable fixtures or production locations;
3. validate the exact prepared-source114 candidate identity while materializing its migration registry from immutable candidate-source bytes;
4. materialize the retained helper from the independent clean controller commit;
5. capture a disposable predecessor, generate candidate and rollback plists, construct the complete closed manifest shape with the exact build-defined timeout budgets, and classify that manifest with the production classifier;
6. call the same read-only helper pre-disruption validator used by real activation;
7. remove the generated root after any failed preparation without mutating the external artifact; and
8. return diagnostic evidence only, never an activation, claim, cleanup, or mutation capability.

The helper pre-disruption validator SHALL be separate from target preparation, capacity reservation, database backup, service control, migration, publication, health activation, claim release, and recovery. It SHALL reject altered PreparedSource114 timeout budgets or plist label/program bindings and leave retained evidence unchanged. Validation success SHALL NOT imply that any production operation is admissible.

## Consequences

- Installer and retained-helper validation cannot silently diverge.
- Exact PreparedSource114 fields remain closed and source-bound; the boundary does not create a generic prepared-114 path.
- The boundary may create and fsync disposable staging evidence and a disposable source114 database, but it cannot acquire production claims or mutate an installed service or application database.
- Production controller ownership, installer adoption, service disruption, migration, publication, cleanup, guardian replacement, and cross-boot recovery remain separately gated decisions.
- macOS qualification can execute the boundary before any native-controller build or process journey, providing executable evidence that preparation remains nonproduction and pre-disruption.

## Rejected alternatives

### Add a dry-run flag to production activation

Rejected because a boolean branch inside an authority-bearing operation leaves mutation reachable by discipline rather than structure.

### Mock helper validation in installer tests

Rejected because duplicated validation can drift from `Manifest.load`, closed-mode validation, identity checks, registry checks, and database-ledger admission.

### Invoke the production deploy command against temporary paths

Rejected because production claim and helper orchestration are not parameterized as a disposable authority domain and therefore cannot be safely inferred from path substitution.

### Move installer-owned migration or publication authority into the controller

Rejected because this milestone validates the boundary; it does not transfer ownership of irreversible installer operations.


## References

- [ADR-125](125_qualify-exact-prepared-source114-candidate.md)
- [ADR-133](133_standalone-controller-owns-durability-and-supervision.md)
- [ADR-135](135_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md)
- [Standalone deployment controller requirements](../standalone-deployment-controller/requirements.md)
- [Standalone deployment controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium)
- [Standalone deployment controller status](../standalone-deployment-controller/executive.md)
