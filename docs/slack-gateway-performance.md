# Slack gateway scheduling measurements

## Controlled backlog comparison

The fixture runs production parsing, discovery and SQLite-backed acceptance.
It contains six recorded baseline sources followed by one unseen reaction on
an older eligible message; the prefix query is empty. Each search page contains
one result. Calls count logical tool dispatches rather than provider-internal
HTTP requests or transport retries.

| Measurement | One page per query/pass | Bounded bursts and early deduplication |
| --- | ---: | ---: |
| Discovery passes to acceptance | 7 | 1 |
| Total calls | 36 | 12 |
| Search calls | 8 | 8 |
| Source/context reads | 14 | 2 |
| Reaction reads | 7 | 1 |
| Owner profile reads | 7 | 1 |

Passes run without real interval waits. The removed six scheduled waits imply
about six minutes for this fixture at a minute cadence; this is not a measured
live latency speedup. Larger search pages have not been introduced.

## Delivery and idle behavior

Native conversation fixtures exercise intentional reply selection, the durable
outbox and post-commit wakeup. Two queued replies use five logical calls and no
searches. Local timings exclude Slack network time. One hundred idle recovery
checks make no MCP calls; production recovery checks occur every two seconds.

A private live timing sample before newest-prefix refresh took 120.255 seconds
from posting to reply: 114.161 seconds to acceptance, 5.943 seconds to the reply
commit, and 0.151 seconds from commit to the Slack posted timestamp. The first
segment includes 10.938 seconds before discovery was enabled. The retained scan
had already completed prefix search and needed another scheduled sweep. One
request, one native input and one reply were observed; discovery was paused
again after a duplicate-check poll. This sample does not establish a latency
distribution or a matched before/after comparison. Personal source identifiers
and operational records are retained outside the public repository.

## Newest-prefix refresh

The additional head probe reads one newest prefix page only when a retained
prefix position has advanced and initialization is complete. It shares exact
verification, accepted-source deduplication, provider backoff and the same call
allowance as ordinary catch-up. It needs eleven remaining calls: at most three
for its page, four for a catch-up page and four reserved for accepted work.
First opportunity alternates between refresh and saved work. Probe evidence
cannot advance saved cursors or establish full-window/baseline completion.

Qualification records the focused scenarios and observed call counts in the
feature executive summary. A failed probe may leave other saved pages able to
progress; a throttled probe prevents further Slack calls until the deadline.
The probe deliberately ignores its next cursor. Additional new messages and
late indexing remain the full sweep's responsibility.
