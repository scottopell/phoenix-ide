# Local Slack gateway

The gateway uses an existing authenticated Slack MCP connection. Agents execute
locally as ordinary Phoenix conversations under the server user's permissions.

## Configure while discovery is disabled

Open Settings → Slack gateway (also available at `/settings/slack`). Configure
an existing MCP server, your Slack user ID, the exact workspace permalink host,
your IANA timezone and the Phoenix origin, such as `https://phoenix.example`
or `https://localhost:8031`, without a path, query or fragment.
Slack owner IDs beginning with either `U` or `W` are supported.
The read-access check verifies identity
and source access without accepting requests or changing Slack.
It shares the gateway's call allowance and provider retry deadline, so an
exhausted allowance or active retry delay can postpone the check.

Finish outstanding work before switching the configured owner or workspace.
Pausing discovery still lets accepted work and its deliveries finish.

Optional private notifications use an owner-configured executable. Configure
its absolute path in settings, or an executable-plus-arguments JSON array in
`notification_command` through the settings API. Keep provider credentials and
personal routing outside this repository. See [notification protocol](#notification-protocol).
Missing notification configuration leaves private notices visibly pending.

## Gestures and initialization

- React fire to someone else's message: create a separate private briefing.
- Start your message with fire: execute the explicit request and reply in its thread.
- React fire to your own message: submit that message as an explicit request.
- Eyes means durable acceptance; it does not claim completion or delivery.

Initialization records historical matches without starting agents. Wait for
ready before using triggers; gestures observed during initialization can be
absorbed by the baseline. Searches cover today and the previous three calendar
dates in your timezone. Leave fire attached until eyes appears. Removing fire
after acceptance does not cancel native work.
Set the timezone before initialization; once its first sweep has started,
changing the timezone requires that initialization finish first.

Same-thread explicit requests reuse one ordinary conversation. Untriggered
Slack replies do not wake it. Native questions return to Slack and require a
new explicit trigger to answer. Phoenix follow-ups remain private unless an
intentional Slack answer is selected for a new Slack request.

Discovery runs once per minute. A bounded newest-prefix-page check can find a
new explicit request while an older sweep continues. Search indexing, saved
pagination, time bounds, provider backoff and the shared 16-call/60-second
allowance all affect latency. No fixed response deadline is promised. Reaction
occurrence timestamps are unavailable, and reactions on out-of-window messages
are not discoverable. External shared-channel sending depends on transport
support; an unavailable destination is surfaced rather than silently replaced.
Replies retry a rate limit only when structured evidence identifies rejection
of `chat.postMessage`. An opaque error or failed later permalink lookup stays
uncertain, since the post may already exist.

## Notification protocol

The executable receives one JSON document on stdin and returns one JSON document
on stdout. Arguments are fixed by owner configuration, never supplied by the
agent. The command is frozen before a submission; status reads use that exact
command and receipt even after settings change. Exit failure, timeout or invalid
submission evidence leaves the external outcome uncertain; Phoenix does not
blindly retry a send. Stderr is not exposed as notification content.
A correlated `failed` submission with a nonempty sanitized `reason` is a
terminal failure, retained with its receipt rather than treated as uncertain.
A command that could not be launched is definitely unsent and remains retryable
after the executable is repaired. Once a command starts, ambiguous outcomes
retain the conservative uncertainty rule.

Submit input:

```json
{"version":1,"operation":"submit","notification_id":"notice-id","owner":"UOWNER","text":"Phoenix: your briefing is ready","url":"https://phoenix.example/c/conversation-id"}
```

Submission response:

```json
{"version":1,"notification_id":"notice-id","owner":"UOWNER","receipt":"opaque-receipt","state":"submitted"}
```

Inspect input:

```json
{"version":1,"operation":"inspect","notification_id":"notice-id","owner":"UOWNER","receipt":"opaque-receipt"}
```

Inspection returns the same version, notification ID, owner and receipt. State
is `submitted`, `delivered` with nonempty `proof`, or `failed` with a sanitized
`reason`. The adapter must verify its actual recipient and exact delivery
operation before returning proof; a service accepting a request is insufficient.
It must not echo credentials, private findings, raw provider headers or request
bodies. The process timeout is 45 seconds and output is capped at 64 KiB.

For a source inside a thread, acceptance verifies the parent and exact reply
together using a narrow timestamp range. Its initial quoted context includes
those two messages; the agent can read more discussion through ordinary Slack
tools when the assignment needs it. Top-level sources retain a bounded thread
read. Coverage and truncation remain visible in the conversation.
Confirmation does not prove that the owner's device displayed an alert.

## Acceptance and stopping

Use a concrete, separately approved private test plan before live integration
writes. Verify exact source/native/return correlation and deduplication, then
leave discovery enabled or pause it according to the owner's activation choice.
Keep only one Phoenix instance watching the same owner.
Disabling discovery stops new acceptance while accepted work continues.
Unknown external outcomes remain visible for inspection rather than resend.
Production activation must qualify its actual installed schema transition and
preserve installed configuration and credentials. See the feature executive
summary for current qualification status.
