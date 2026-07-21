# Replace derived WorkScope identity with durable scope ownership

Replace path/transcript-derived resource identity with one opaque durable `WorkScopeId`. Make each WorkScope own its allocated-worktree, unowned-directory, or no-filesystem environment; preserve conversation-addressed durable delivery; and cut scope-owned resources and PR associations over without dual writes.

This task includes the bounded Projects spEARS v2 migration, schema/backfill/cutover, conversation/continuation/sub-agent creation invariants, resource actor-authorization boundaries, explicit WorkScope retirement, deletion of Direct-continuation rekey/fallback machinery, and migration/property/integration verification.
