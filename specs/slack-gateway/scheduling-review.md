# Scheduling model review

This is a point-in-time review record. The model has two layers: durable source/
request/return authority, and bounded discovery/service opportunities. An
allowance grant is necessary for a Slack call but does not authorize a task,
select a public response or confirm delivery.

| Counterexample | Required behavior |
| --- | --- |
| Repeated delivery wakeups | Share the unexpired process-local window; no allowance refill per wakeup. |
| Window expires while delivery is pending | Any new service opportunity can renew an expired window. |
| Incomplete candidate verification | Retain the page; do not record an unseen source or prove completion. |
| Disable races transactional acceptance | Distinguish recorded evidence from inactive rejection before advancing. |
| Adding eyes is throttled | Retain retry deadline and visible backoff, including when search cannot start. |
| New prefix appears behind an advanced saved scan | Offer one bounded head probe using the ordinary verification/acceptance boundary. |
| Head and catch-up find the same source | One immutable accepted identity, admission and response obligation. |
| Too little allowance for head plus catch-up and delivery | Skip the head probe, preserve saved-page service. |
| Fresh read fails or takes too long | Keep saved positions intact; alternate first opportunity and honor shared backoff/deadline. |
| Gateway is still initializing | No special freshness acceptance path. Only a completed full baseline enables acceptance. |
| Several new prefixes or late indexing | The head probe makes no completeness promise; full catch-up remains responsible. |
| Notification settings change after submission | Inspect the frozen command/receipt; never resend an uncertain operation. |

Structural checking is not a formal temporal proof. Eventual completion depends
on usable provider pagination and eventually successful verification or positive
rejection. Persistent inaccessible candidates may retain their pages. Exact PR
review and production/private-acceptance evidence are tracked in executive.md.
