# ADR-281: Retain the exact installed predecessor with deployment recovery evidence

- **Status:** Accepted
- **Date:** 2026-11-24
- **Affects:** REQ-LDD-032, REQ-LDD-034, REQ-COMP-023

## Context

On 2026-11-24, RC13 production recovery assessment found that successful launchd deployment retained its verified database backup and predecessor hash/identity proof but deleted the transaction-owned exact predecessor executable. Retained signed/notarized distribution bytes were not the actual installed predecessor: installation ad-hoc signs the candidate and binds the resulting installed hash. Source identity alone could not establish the claimed exact retained binary/database recovery pair.

The delivery owner commissioned a narrow retention correction and immediate private preservation of existing installed RC13 bytes, without service mutation, new database snapshots or a recovery framework. Capacity and backup decisions remain separate gates.

## Options considered

1. Retain distribution bytes and source identity only: insufficient to recover the exact installed executable after installation signing changes its bytes.
2. Retain and verify the existing transaction-owned predecessor executable and transaction-local proof: uses the existing mechanism and preserves the exact binary/database binding without automatic recovery.
3. Add a separate archive or rollback framework: more lifecycle machinery than the identified retention defect requires.

## Decision

Implement REQ-LDD-034 and REQ-COMP-023 in the existing transaction directory/proof. Before disruption, an authenticated predecessor requires a regular non-symlink `previous-phoenix-ide` matching the freshly captured installed predecessor SHA-256. Fsync the file/directory and record its path alongside existing predecessor identity/hash in proof. Retain it after success as well as unresolved failure; clean only the temporary candidate on commit. The verified stopped-database backup remains in the same transaction with its existing witness. Persist authoritative proof beside the transaction candidate before updating the shared latest-status proof projection. Later deployment attempts must not overwrite an earlier transaction's retained proof. No new archive mechanism is needed.

First install and unauthenticated predecessors do not claim authenticated recovery pairs. Helper replay remains refused; restart leaves retained deployment artifacts unchanged. Manual matching offline recovery remains the operator contract; no automatic postboundary predecessor startup is introduced.

## Consequences

Each qualifying transaction retains roughly one additional installed executable. This is small compared with the database backup but must be included in lifecycle budgeting. Existing historical transactions are not silently repaired or reclassified. Their retained distribution binaries can be assessed separately on disposable data, but exact-byte match cannot be claimed before measurement.

Published macOS release deployment materializes its helper from the selected release commit; separate controller-source pinning is restricted to Linux systemd. Therefore this correction requires a newly qualified supported release for macOS published-release delivery rather than changing immutable RC14 or borrowing a newer helper for its old release identity.

Validation covers success retention, refusal of stale/missing/symlink saved bytes before disruption, failure evidence/manual recovery, replay refusal and artifact-preserving restart on disposable fixtures. This does not establish production restore acceptance or an archival/pruning policy.
