# Replace derived WorkScope identity with durable scope ownership

Implement Slice 1 of the approved conversation-authority architecture after PR #532: migrate Phoenix from path/transcript-derived WorkScope identity to one opaque durable WorkScopeId, normalize runtime role and allocated/unowned/no-environment state, preserve conversation-addressed durable delivery, and cut all scope-owned resources and PR associations over without dual writes.

This task includes the bounded Projects spEARS v2 migration, schema/backfill/cutover, conversation/continuation/sub-agent creation invariants, resource actor-authorization boundaries, explicit WorkScope retirement, deletion of Direct-continuation rekey/fallback machinery, and migration/property/integration verification. It does not implement the authority-request UI/backend yet.
