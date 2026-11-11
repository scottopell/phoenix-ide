# ADR-248: Slack emoji aliases share discovery and reinitialize baselines

- **Status:** Accepted
- **Date:** 2026-11-11
- **Affects:** REQ-SG-002, REQ-SG-004, REQ-SG-005, REQ-SG-018

## Context

The owner wants dedicated custom emojis instead of a social fire reaction, and
multiple interchangeable names. Search cursors describe the query that produced
them. Reusing them after changing names can skip matches or admit old social
reactions as new requests.

## Options considered

1. **Independent discovery and baselines per emoji.** Retains fine-grained
   catch-up but multiplies query progress, initialization, and scheduling state.
2. **Combined OR queries and a fresh baseline when names change.** Keeps the two
   existing search streams and source identity; setup briefly suspends admission.
3. **Change filters without resetting progress.** Smallest patch, but cursors and
   baseline evidence no longer describe the active query.

## Decision

Use combined OR queries for prefixes and owner reactions. All names invoke the
same three workflows. Store the names as constrained rows, defaulting to `fire`;
keep their presentation order separate from effective set equality.

A changed set discards retained discovery positions and starts a new baseline.
Updates serialize with active gateway passes. Handled source identities, accepted
work, replies, and provider backoff survive. Reordering the same set does not
restart initialization.

## Consequences

The gateway keeps one stable request identity per Slack message and does not add
polling loops per emoji. Pre-existing matches and gestures during initialization
can be absorbed without execution, including previously unseen matches for retained
names. Settings explain this pause and the need to wait for healthy discovery.
Uploading custom emoji images remains a Slack action, outside gateway configuration.

## References

- [Slack gateway requirements](../slack-gateway/requirements.md)
- [Discovery behavior](../slack-gateway/discovery.allium)
