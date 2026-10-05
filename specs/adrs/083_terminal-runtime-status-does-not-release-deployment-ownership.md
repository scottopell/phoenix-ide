# ADR-083: Terminal runtime status does not release deployment ownership

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** launchd deployment, release updates, compatibility

## Context

ADRs 081–082 distinguish verified predecessor resume and candidate publication completion from claim-removal durability. A terminal runtime receipt can precede interrupted claim removal, so interpreting its state alone as approval-ready loses actual ownership. Completed ordinary migration transactions also contain private database copies; excluding all such manifests from existing pruning retains several database sizes forever, even after matching recovery is completed.

## Options considered

1. Redefine runtime terminal states or add a new recovery scheduler: rejected; runtime outcome and host ownership are different facts, and existing explicit recovery/bounded pruning suffice.
2. Infer ownership release from terminal state: rejected; interrupted unlink/fsync can retain the claim.
3. Project actual ownership separately and retain durable per-transaction completion evidence for existing pruning: selected.

## Decision

The current release status API derives optional retained ownership from the actual backend active claim; unreadable evidence conservatively fences. Current UI terminal interpretation requires both runtime finalization and ownership release. Terminal-with-claim outcomes warn, poll, block approval/reconciliation and show matching recovery guidance, without describing manual-resume claim cleanup as candidate publication. The approval endpoint independently refuses actual retained ownership. Older APIs omitting the optional field retain their established interpretation; this does not update a captured predecessor's embedded UI.

After ordinary migration terminal completion and durable claim removal, the helper writes a private transaction-local completion receipt. Existing bounded deployment pruning admits only completed claim-free ordinary transactions with matching receipt; active, failed, pending and unproven transactions remain protected. Pruning deletes only transaction-owned copies, never external operator originals. Missing receipt after interruption remains conservative and can be recorded by existing verified terminal retry; no backup-retention guarantee or new scheduling platform is added.

## Consequences

Candidate-committed and manually-resumed release failures share actual ownership truth without adding new runtime states. Terminal-state/claim-present/claim-absent matrices and the real disposable release-failure/retry lifecycles feed API/UI tests. Proven completion no longer permanently exempts private copies from normal bounded retention, while unresolved recovery evidence stays owned.
