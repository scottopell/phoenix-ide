# Slack gateway scheduling measurements

## Controlled backlog comparison

The fixture runs production parsing, discovery and SQLite-backed acceptance.
It contains six recorded baseline sources followed by one unseen reaction on
an older eligible message; the prefix query is empty. Each search page contains
one result. Calls count tool dispatches rather than provider-internal HTTP
requests. MCP recovery does not replay gateway calls; a later retry consumes
another allowance grant.

| Measurement | One page per query/pass | Initial burst optimization | Current connection verification |
| --- | ---: | ---: | ---: |
| Discovery passes to acceptance | 7 | 1 | 2 |
| Total calls | 36 | 12 | 16 |
| Search calls, including identity-only probes | 8 | 8 | 11 |
| Source/context reads | 14 | 2 | 2 |
| Reaction reads | 7 | 1 | 1 |
| Owner profile reads | 7 | 1 | 2 |

The initial optimization predates independent workspace verification. Current
active passes use a profile read plus one identity-only permalink lookup on a
fixed MCP connection. Two such probes and the additional pass's freshness check
are included above. The four-call work reserve prevents starting the last unseen
page in the first window; it completes in the second pass. Source-read savings
remain unchanged. Empty workspace evidence refuses writes rather than guessing.

Passes run without real interval waits. At a minute cadence the current fixture
removes five scheduled waits; this is not a measured live latency speedup.
Larger search pages and caching identity across passes have not been introduced.

## Delivery and idle behavior

Native conversation fixtures exercise intentional reply selection, the durable
outbox and post-commit wakeup. Two queued replies use six logical calls and no
trigger searches, including the identity-only workspace lookup. Local timings
exclude Slack network time. One hundred idle recovery
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

## Notification inspection and acknowledgments

Private notification inspections use a separate persisted thirty-second minimum
interval after pending results or read errors. Frequent local recovery passes
do not start another adapter command before that deadline. The local fixture
performs one inspection across four immediate recovery passes, then one more
after advancing the deadline; this measures logical adapter calls, not live
provider requests. Terminal source-loss acknowledgment evidence also prevents
repeated impossible add-reaction calls without changing the accepted task.
Ready thread replies are dispatched before submitted-notice inspection. Each
service pass starts at most one such inspection, ordered by the earliest retained
deadline. A trace fixture with two older submitted notices and a newer ready
reply sends the reply first, inspects one notice, then inspects the other on the
next pass. The initial reservation covers the 45-second maximum command duration
plus thirty seconds, so a failed completion write cannot permit an early retry.

## Newest-prefix refresh

The additional head probe reads one newest prefix page only when a retained
prefix position has advanced and initialization is complete. It shares exact
verification, accepted-source deduplication, provider backoff and the same call
allowance as ordinary catch-up. It needs twelve remaining calls: at most four
for its page, four for a catch-up page and four reserved for accepted work.
First opportunity alternates between refresh and saved work. Probe evidence
cannot advance saved cursors or establish full-window/baseline completion.
The fresh candidate's exact source and context reads share that bound. A possible
connector reaction footer also needs independent reaction metadata; unambiguous
owner prefixes skip that read. A literal reaction-shaped instruction line is preserved.

Qualification records the focused scenarios and observed call counts in the
feature executive summary. A failed probe may leave other saved pages able to
progress; a throttled probe prevents further Slack calls until the deadline.
The probe deliberately ignores its next cursor. Additional new messages and
late indexing remain the full sweep's responsibility.

## Local production pilot

Two sequential explicit requests in one private thread produced exact intentional
replies in 34.277 and 10.364 seconds from Slack's posted timestamps. Each had
one native input and one return; both reused the same native conversation.
A managed restart retained their identities and confirmed returns without
duplicate work. The provider-neutral notifier also established delivery to the
configured owner, who confirmed receiving the alert. Reacting to the notice
created one separate private briefing and an exactly correlated confirmed private
return, without a source-thread reply. These checks establish the bounded
interaction, rather than a latency measurement for reaction discovery.

These are two end-to-end observations with different polling phases, not a
matched performance comparison, percentile or response-time guarantee. Source
identifiers, private provider receipts and deployment evidence remain outside
the repository. Per-pass call metrics exclude calls charged earlier to the
shared fixed window.
