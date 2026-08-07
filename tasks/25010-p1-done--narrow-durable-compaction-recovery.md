# Narrow durable compaction recovery to one lifecycle

Remove permanent legacy half-commit startup reconciliation and general-purpose acknowledged-event/lifecycle machinery introduced while fixing context compaction. Preserve durable continuation identity, atomic start/commit, stale-result rejection, normal pending-operation restart recovery, explicit retry, provider-authoritative exhaustion, UI/CLI visibility, and state_kind.

Validate provider failure/retry, restart resume, duplicate/ambiguous commit, retry admission acknowledgement, and provider-authoritative dispatch behavior.
