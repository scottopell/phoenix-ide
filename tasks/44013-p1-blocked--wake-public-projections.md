# Wake public projections and exhaustive event registry

Depends on tasks 44011 and 44012 and their merged PRs.

Project the authoritative wake aggregate into API inspection, durable transcript delivery, live SSE, replay, generated TypeScript, and UI hydration/status. Introduce one exhaustive public-event registry that drives Rust wire labels/conversion, replay class, sequence-barrier behavior, generated types/manifest, TS schema/router exhaustiveness, and UI reducer/cache invalidation obligations.

Prove atomic message/lifecycle/barrier ordering, reconnect hydration equivalence, fast register+terminal visibility, stale-response suppression, projection idempotency, and exactly-once public materialization. Projections are rebuildable and never become a second wake authority.
