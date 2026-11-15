# ADR-249: Slack uses one configurable trigger emoji

- **Status:** Accepted
- **Date:** 2026-11-11
- **Affects:** REQ-SG-002, REQ-SG-004, REQ-SG-005, REQ-SG-018

## Context

Live exclusion checks show that the Slack MCP search tool ignores grouped OR
expressions in its filters. The broadened searches exhaust connector pagination.
The owner prefers one dedicated custom emoji over the cost of multiple query
positions, freshness rotation, and additional search traffic.

## Options considered

1. Search separately for each emoji and merge verified sources. This preserves
   aliases but adds paging and scheduling state and consumes more calls.
2. Configure one emoji. This preserves the three workflows and custom naming
   with the existing two supported searches.

## Decision

Replace the emoji list with one required `trigger_emoji` string in the API and
one scalar application setting. Remove list controls, OR query construction,
set comparisons, and alias matching. The three workflows and source identity
remain unchanged. Name changes establish a new baseline; equivalent normalized
names preserve progress.

Migration from the published list schema retains its first saved name, drops
the list table, and discards discovery positions only when that list had multiple
names. Those positions describe unsupported OR searches. Baseline readiness,
accepted work, handled identities, observations, and provider backoff survive the
upgrade. An unfinished baseline remains unfinished. No runtime array API alias
is retained; stale clients must reload.

## Consequences

Only one emoji can trigger Phoenix at a time. Configuring a custom emoji disables
fire. Existing single-emoji production settings and paging continue unchanged.
This removes the multi-emoji cause of broad queries; it does not promise unlimited
connector paging for a genuinely large single-emoji backlog.

## References

- Supersedes [ADR-248](248_slack-emoji-aliases-share-discovery-and-reinitialize-baselines.md).
- [Slack gateway requirements](../slack-gateway/requirements.md).
