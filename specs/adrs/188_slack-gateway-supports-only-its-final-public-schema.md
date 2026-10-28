# ADR-188: Slack gateway supports only its final public schema

- **Status:** Accepted
- **Date:** 2026-10-27
- **Affects:** REQ-SG-005/011/012/015/017; REQ-COMP-002/020

## Context

The Slack feature is unmerged. Its prototype builds used several incompatible
schemas and behaviors. Keeping their conversion paths adds runtime branches,
obsolete storage and tests for behavior the public feature does not produce.
The deployed pilot already requires separate offline upgrade qualification.

## Options considered

1. Keep prototype migrations and interpretation of their old questions, trigger
   provenance, notification retries and inspection metadata.
2. Install only the final schema and reject prototype identities through the
   existing migration-lineage preflight.

## Decision

Choose option 2, as requested by the owner. Preserve upstream migration history
through 140 and introduce one migration, `141/slack_gateway_v1`, containing the
final Slack schema. Its distinct identity prevents an earlier unmerged schema
from masquerading as the final schema. No online prototype conversion is provided.

Remove obsolete storage, runtime interpretation and prototype compatibility
tests. Keep current restart recovery, thread reuse, exact reply coverage and
single-attempt signals. This supersedes ADR-155's prototype conversion policy.

## Consequences

- **Positive:** No compatibility subsystem for unmerged Slack formats.
- **Negative:** Prototype databases cannot be opened as final public databases;
  preserving their work requires a separately qualified offline upgrade.
- **Neutral:** The deployed pilot is untouched. Upstream migration history and
  the protected production-deployment gate remain unchanged.

## References

- [Slack gateway requirements](../slack-gateway/requirements.md)
- [Compatibility requirements](../compatibility/requirements.md)
- ADR-034, ADR-155, ADR-187
- `lineage::preflight`, `run_pending_migrations`, `slack_recover_returns`

## Integration allocation

Main published `141/canonical_tool_round_receipts` before this feature merged.
The final Slack schema is therefore allocated `142/slack_gateway_v1`; upstream
migrations through 141 remain unchanged. The prototype refusal policy is unchanged.
