# Bounded Close FK-787 repair and cleanup authority recovery

Extract onto current main only the exact-attempt cleanup authority adoption and legacy SQLite FK-787 retry reconciliation needed to complete the retained Close quarantine safely. Include exact-generation rollback/idempotency/live-identity regressions, Linux per-task cwd fail-closed inspection, and exact-scope tombstone error routing. Preserve task 33001 / PR #764 as historical evidence; do not reactivate or merge it wholesale.

No new claim endpoint, production database surgery, manual quarantine deletion, or deployment belongs to this source task. Production cleanup remains gated on a separately qualified deployment and the sole host executor's fresh exact-target checks.
