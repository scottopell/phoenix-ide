# Saved Goals API

A saved Goal records an objective and its status for an ordinary conversation.
**Saving or changing a Goal does not dispatch work, resume an agent, or stop one.**
Assign work through the conversation separately. `Suspended` describes Goal
metadata, not the runtime's execution state.

## Authenticate and choose a conversation

Sign in to Phoenix as its owner, then use the browser's DevTools Console on that
same Phoenix origin. These examples reuse the HttpOnly owner-session cookie;
never paste credentials into a conversation. Share-link access is not owner
access. Commands below change saved metadata: choose the intended conversation
and edit the objective before running them.

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
  const data = await response.json();
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
const before = await api(goal);
console.log(before);
console.log(await api(`${goal}/history?after=0`));
```

Current state is `null` when there is no Goal history; otherwise it is an entry
containing `snapshot.version`, `snapshot.goal_id`, `snapshot.objective`, and
`snapshot.state`. History is an ordered array of entries; `after=N` returns
versions strictly greater than N. Versions belong to the conversation's Goal
aggregate, not separate counters for individual Goals.

## Create a Goal

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
console.log(await api(`${goal}/history?after=0`));
```

Disposition JSON is `"Underway"`, `{"Blocked":"reason"}`,
`{"Suspended":"reason"}`, `{"Complete":"reason"}`, or
`{"Exhausted":"reason"}`. Non-Underway reasons must be nonempty. Complete and
Exhausted are terminal; a state-change request cannot reopen them.

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
  than accepted Goal changes. Without owner authentication, requests are refused;
  sign in normally rather than bypassing authentication.

The supported routes and request fields are defined in
[`goal_commands.rs`](../crates/phoenix-ide/src/api/goal_commands.rs); disposition
and history shapes are in
[`conversation_goal.rs`](../crates/phoenix-core/src/domain/conversation_goal.rs).
