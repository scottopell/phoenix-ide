# Local Slack gateway

The gateway uses an existing authenticated Slack MCP connection. Agents execute
locally as ordinary Phoenix conversations under the server user's permissions.

## Configure while discovery is disabled

Open Settings → Slack gateway (also available at `/settings/slack`). Configure
an existing MCP server, your Slack user ID, the exact workspace permalink host,
your IANA timezone and the Phoenix origin, such as `https://phoenix.example`
or `https://localhost:8031`, without a path, query or fragment.
Slack owner IDs beginning with either `U` or `W` are supported.
The read-access check verifies identity and samples both discovery queries
without accepting requests or changing Slack. It reports sampled hits and
verified sources; empty or unverified samples are inconclusive about source
access. Reaction access is reported only after an actual successful read.
It shares the gateway's call allowance and provider retry deadline, so an
exhausted allowance or active retry delay can postpone the check.

Finish outstanding work before switching the configured owner or workspace.
Pausing discovery still lets accepted work and its deliveries finish.

Optional private notifications use an owner-configured local command. The settings
summary shows whether it is configured; expand “Edit notification command” to
view or edit its program and every argument. A launcher such as uv is distinct
from the notification script passed in its arguments. Configure
the absolute executable path there, or an executable-plus-arguments JSON array in
`notification_command` through the settings API. Keep provider credentials and
personal routing outside this repository. See [notification protocol](#notification-protocol).
Each private notice has one bounded command attempt. Missing configuration or
launch rejection fails visibly; ambiguous or interrupted attempts remain uncertain.
There are no automatic retries or receipt inspections, including after restart.
An accepted submission does not prove alert delivery. Work remains in Phoenix.

## Gestures and initialization

- React fire to someone else's message: create a separate private briefing.
- Start your message with fire: execute the explicit request and reply in its thread.
- React fire to your own message: submit that message as an explicit request.
- Eyes means durable acceptance; it does not claim completion or delivery.

Initialization records historical matches without starting agents. Wait for
ready before using triggers; gestures observed during initialization can be
absorbed by the baseline. Searches cover today and the previous three calendar
dates in your timezone. Leave fire attached until eyes appears or acceptance is visible in Phoenix. Removing fire
after acceptance does not cancel native work.
Eyes is attempted once after durable acceptance. Failure or interruption remains
visible without retry; missing eyes does not mean the request was missed.
The accepted task and its response continue independently.
Set the timezone before initialization; once its first sweep has started,
changing the timezone requires that initialization finish first.

Status separates observed-to-accept delay from prefix-message age at observation.
Message age is shown only for verified prefix triggers; historical requests with
unknown trigger provenance do not acquire a guessed measurement.

Same-thread explicit requests reuse one ordinary conversation. Untriggered
Slack replies do not wake it. Clarifications return to Slack with an
“Open in Phoenix” deep link: answer the question in that Phoenix conversation.
Slack messages and reactions do not answer native questions. While a question
blocks ordinary admission, new explicit requests receive an explanation and
the Phoenix link. The clarification completes the automatic Slack response;
further discussion in Phoenix remains private. Send a new explicit Slack
request when you want another intentional answer published there.
Clarification messages contain only a generic handoff and Phoenix link. Question
text, options and private answers stay in Phoenix. Changing or answering the
question does not require replacing the handoff.

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
Thread returns have at most three claimed attempts, including pre-dispatch
failures, across restart. Exhaustion records a delivery failure and queues one
best-effort private notice; Phoenix retains the result and failure details.
The same proof is required before declaring a destination unavailable. If final
delivery persistence fails after a send, a later service pass exposes the attempt
as uncertain without sending it again or requiring a Phoenix restart.

## Notification protocol

The local command receives one version-1 JSON request on stdin:

```json
{"version":1,"operation":"submit","notification_id":"notice-id","owner":"UOWNER","text":"Phoenix: your briefing is ready","url":"https://phoenix.example/c/conversation"}
```

Return one correlated response on stdout with a nonempty submission receipt:

```json
{"version":1,"notification_id":"notice-id","owner":"UOWNER","receipt":"opaque-receipt","state":"submitted"}
```

A rejected submission uses `"state":"failed"` and a nonempty `reason`.
The receipt is historical submission evidence. Phoenix never invokes an `inspect`
operation and never infers recipient delivery from command acceptance. Existing
adapters that accept `submit` and return this response remain compatible.

Execution is bounded to 45 seconds and 64 KiB stdout. A launch failure is recorded
as failed; timeout, malformed evidence or interrupted execution is uncertain.
Neither is automatically retried. The attempt is persisted before launch, so a
crash in between may produce no alert. Keep provider credentials and recipient
routing in the local adapter; send only a generic notice and Phoenix link.

The command is optional. Without it, alert failures are visible in Activity and
native work continues. Account changes wait for native work and source-thread
duties, rather than notification or acknowledgment success. Pending old-account
private notices are visibly skipped when its account changes.

For a source inside a thread, acceptance verifies the parent and exact reply
together using a narrow timestamp range. Its initial quoted context includes
those two messages; the agent can read more discussion through ordinary Slack
tools when the assignment needs it. Top-level sources retain a bounded thread
read. Coverage and truncation remain visible in the conversation.
Notification submission does not prove that the owner's device displayed an alert.

## Acceptance and stopping

Use a concrete, separately approved private test plan before live integration
writes. Verify exact source/native/return correlation and deduplication, then
leave discovery enabled or pause it according to the owner's activation choice.
Keep only one Phoenix instance watching the same owner.
Disabling discovery stops new acceptance while accepted work continues.
Unknown external outcomes remain visible for inspection rather than resend.
Source-verification failures remain visible in settings and retain their page
for retry. Gateway status offers a conversation link only while native routing
can open the published conversation; pending or failed initial delivery and deleted targets remain inspectable
on the status page.
Production activation must qualify its actual installed schema transition and
preserve installed configuration and credentials. See the feature executive
summary for current qualification status.

## Database boundary

The public feature installs the final `141/slack_gateway_v1` schema on a supported
canonical Phoenix database. Earlier unmerged Slack databases are unsupported and
are refused by migration identity before mutation. Preserve prototype work for a
separate offline upgrade plan; do not relabel its ledger or open it as a fresh
installation. The normal protected production-deployment gate still applies.
