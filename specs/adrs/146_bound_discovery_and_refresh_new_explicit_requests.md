# ADR-146: Bound discovery and refresh new explicit requests

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-SG-003, REQ-SG-017; discovery model

## Context

A retained scan can span many polling intervals. Restarting at the head loses
catch-up progress; finishing a large scan first delays newly indexed requests.
Verification, acknowledgments and returns share the same MCP call allowance.

## Options considered

1. Finish every saved scan before looking at newest explicit messages.
2. Restart saved queries each interval, risking starvation of older pages.
3. Add a bounded fresh prefix probe while retaining catch-up cursors.

## Decision

Use one shared fixed window of sixteen calls per sixty seconds, with observed
backoff, a soft discovery time bound and an accepted-work reserve. Deduplicate
known source evidence before redundant context reads. Persist normal query
cursors and completion independently of accepted source records.

After readiness and prefix progress, offer one newest-prefix page when at least
eleven calls remain: up to three refresh calls, four catch-up calls and four
accepted-work calls. Alternate refresh-first and catch-up-first opportunities.
Fresh evidence uses normal source validation and atomic acceptance but cannot
advance a saved cursor or complete a full scan. Failed probes preserve saved
progress; rate limits constrain all callers.

## Consequences

- New explicit requests can be accepted before a retained sweep finishes.
- Older reaction discovery keeps service opportunities under available time,
  allowance and successful transport operations.
- One fresh page is not a latency guarantee or complete new-message feed;
  indexing and subsequent full pagination still matter.

## References

- `specs/slack-gateway/discovery.allium`
- `discover_ordered`, `refresh_prefix`, `accept_page_sources`, `Slack::can_refresh`
- `docs/slack-gateway-performance.md`
