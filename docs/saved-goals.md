# Saved Goals API

A saved Goal records an objective and its status for an ordinary conversation.
**Saving or changing a Goal does not dispatch work, resume an agent, or stop one.**
Assign work through the conversation separately. `Suspended` describes Goal
metadata, not the runtime's execution state.

## Authenticate and choose a conversation

Use the browser's DevTools Console on the same Phoenix origin, over **HTTPS or
localhost**. The write examples use `crypto.randomUUID()`, which requires a secure
context and is unavailable on non-localhost HTTP origins.

When password authentication is enabled (`PHOENIX_PASSWORD` is set), sign in as
the owner first; these examples reuse the HttpOnly owner-session cookie.
Share-link access is not owner access. When no password is configured, Phoenix
treats incoming requests as owner-authenticated without a sign-in; these routes
do not independently protect a passwordless server. Never paste credentials into
a conversation. Commands below change saved metadata: choose the intended
conversation and edit the objective before running them.

```js
const api = async (path, body) => {
  const response = await fetch(path, {
    credentials: 'same-origin',
    ...(body === undefined ? {} : {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(body)
    })
  });
  const text = await response.text();
  let data;
  try { data = JSON.parse(text); }
  catch { data = text; }
  if (!response.ok) {
    console.error(response.status, data);
    throw new Error(`Request refused (${response.status}); inspect the response above`);
  }
  return data;
};
const conversations = (await api('/api/product-conversations')).product_conversations;
console.table(conversations.map(c => ({
  id: c.product_conversation_id, route: c.canonical_route,
  transcript: c.latest_transcript_row_id
})));
```

Select the **product conversation ID** matching your open ordinary conversation.
Do not substitute its transcript UUID, slug, URL, or an `@conv:` reference.

## Read current state and history

```js
const owner = 'PASTE_PRODUCT_CONVERSATION_ID';
const goal = `/api/product-conversations/${encodeURIComponent(owner)}/goal`;
const readHistory = async () => {
  const entries = [];
  let after = 0;
  while (true) {
    const page = await api(`${goal}/history?after=${after}`);
    entries.push(...page);
    if (page.length < 100) return entries;
    after = page[page.length - 1].snapshot.version;
  }
};
const before = await api(goal);
console.log(before);
console.log(await readHistory());
```

Current state is `null` when there is no Goal history; otherwise it is an entry
containing `snapshot.version`, `snapshot.goal_id`, `snapshot.objective`, and
`snapshot.state`. History is an ordered array of entries; `after=N` returns
versions strictly greater than N, with at most **100 entries per response**.
`readHistory()` advances `after` to the last returned version until a page has
fewer than 100 entries (an exact multiple requires a final empty page). Versions
belong to the conversation's Goal aggregate, not separate counters for individual
Goals. Paging does not freeze history; later mutations can appear in later pages.

## Create a Goal

Before creating or replacing a Goal, explicitly enable **automatic continuation**
for that target conversation. Admission checks the persisted preference in the
Goal-write transaction; OFF returns HTTP409 `goal_requires_auto_continue` without
changing the Goal or enabling the preference. These examples do not change that
setting.

Disabling automatic continuation later preserves the existing Goal and history.
Reads, progress/status updates on an active Goal, explicit Complete/Suspend/
Exhausted safety exits, and exact retries of accepted commands remain available
while OFF. Re-enabling the preference does not reactivate an inactive Goal or
wake, resume, or dispatch work.

This example deliberately stops if a Goal already exists; choose another
conversation rather than inadvertently replacing someone else's objective.

```js
if (before !== null) throw new Error('A Goal already exists; inspect it before changing anything');
const create = {
  request_id: crypto.randomUUID(),
  expected_version: 0,
  objective: 'Write a concise setup guide for this project'
};
const created = await api(goal, create);
console.log(created.result.entry);
// If the response is lost, retry this SAME object, not a new UUID:
// await api(goal, create);
```

An accepted command returns HTTP200 and a receipt with `request_id` and
`result: {outcome: "accepted", entry: ...}`. Creating a Goal records it as
`Underway`; it does not start the objective's work.

## Change status with compare-and-swap

Read the current entry immediately before preparing a new command. Replace the
example disposition with the status you actually intend to record.

```js
const current = await api(goal);
if (current === null) throw new Error('Create a Goal first');
const change = {
  request_id: crypto.randomUUID(),
  expected_version: current.snapshot.version,
  goal_id: current.snapshot.goal_id,
  disposition: {Blocked: 'Waiting for the project owner to confirm the setup steps'}
};
console.log(await api(`${goal}/state`, change));
console.log(await api(goal));
console.log(await readHistory());
```

Disposition JSON is `"Underway"`, `{"Blocked":"reason"}`,
`{"Suspended":"reason"}`, `{"Complete":"reason"}`, or
`{"Exhausted":"reason"}`. Non-Underway reasons must be nonempty. **Suspended,
Complete, and Exhausted are terminal**; a state-change request cannot reopen them,
even by setting `Underway`. Reactivating that work requires an explicit new Goal
identity, with automatic continuation ON; the previous identity and history are
retained.

## Retries and refusals

- Use a fresh UUID for each **new** command. For a transport retry, retain the
  same UUID and the exact body, including `expected_version`. Retries are scoped
  to the owner instance and product conversation and return the retained receipt
  without appending another history entry. Do not reuse a UUID for different work.
- `expected_version` is compare-and-swap: stale versions are refused, not silently
  overwritten. Re-read current state before deciding on a new command with a new
  UUID. The field is not named `base_version`; `singleton_key` is not an API field.
- Durable refusals return HTTP409 with `result.outcome: "rejected"`,
  `result.reason` (`version`, `busy`, or `transition`), and `observed_version`.
  An identical retry retains a rejection too. Re-read after a stale version;
  after a busy refusal, wait for the conversation to settle before a new request.
- HTTP409 can also be an error response rather than a receipt: for example,
  reusing a request UUID with a different body or addressing a closed conversation.
  Inspect the response; do not assume every conflict appended a durable receipt.
- Malformed IDs, invalid text or unknown JSON fields are request errors rather
  than accepted Goal changes. With password authentication enabled, requests
  without owner authentication are refused; sign in normally rather than
  bypassing authentication. Without a configured password, the server admits
  requests as owner-authenticated, as described above.

## Native Coordinator tools

The current Coordinator can use `read_goal`, `prepare_goal`, and `apply_goal` for
ordinary conversation targets. Unlike the HTTP paths above, these tools require
a stable `@conv:<product-conversation-id>` target. Preparation freezes the observed
Goal/version and returns a server-issued `action_reference`; review it, then pass
only that reference to `apply_goal`. Do not generate references or versions.
Exact apply retries return the retained accepted or refused receipt; conflicts
require explicit reconsideration, not automatic reprepare-and-overwrite.

These tools do not dispatch work, Start/Stop execution, set Suspended, or
reopen a Suspended identity in place. Explicit replacement creates a new identity
and retains the old history. Create/replacement still requires automatic
continuation ON; read/history and retained accepted replay do not. Preparation
alone neither changes the Goal nor enables automatic continuation.

The supported routes and request fields are defined in
[`goal_commands.rs`](../crates/phoenix-ide/src/api/goal_commands.rs); disposition
and history shapes are in
[`conversation_goal.rs`](../crates/phoenix-core/src/domain/conversation_goal.rs).
