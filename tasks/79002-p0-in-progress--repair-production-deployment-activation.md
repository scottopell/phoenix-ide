# Repair production deployment activation

Restore a safe transactional production activation contract after a candidate with newer migrations failed verification and rollback also failed. Activation must tolerate real startup latency without false rollback, prevent pre-acceptance conversation/provider/tool side effects, and never leave an older binary running against a newer migrated database unless the matching database backup is restored.

## Acceptance evidence

- Deterministic disposable-harness coverage for slow candidate readiness, slow rollback readiness, exact identity mismatch, post-migration activation failure, pre-acceptance side-effect suppression, durable status transitions, and crash recovery.
- Compatibility, deployment, and restart specifications/ADRs remain authoritative and are updated if the contract changes.
- Focused validation and full `./dev.py check` pass before any live production manipulation.
- Delivery includes an immutable reviewed SHA and a recovery runbook; no production deploy or launchd manipulation occurs without separate authorization.
