# Supported Phoenix HTTP API reference

This reference describes the server API, not a privileged bypass. Use the Phoenix origin for the deployment the user named. Do not guess a localhost URL when the requested target may be remote.

## Authorization without disclosure

1. `GET /api/auth/status` is the safe, unauthenticated capability check. It returns whether authentication is required; it does not return a credential.
2. If authentication is disabled, do not add credentials.
3. If authentication is enabled, use a credential already supplied or explicitly identified by the user for this deployment. For non-browser requests, Phoenix accepts the configured password as `Authorization: Bearer …`. A `phoenix-auth` cookie is an opaque authenticated session token, not the configured password.
4. Keep secrets out of command arguments, shell tracing, files, logs, summaries, and tool output. Prefer an already-populated environment variable expanded inside the scoped shell. Do not inspect the Phoenix database or process environment to hunt for a credential, and do not echo or interpolate a secret into diagnostic output.
5. A `401` or `403` is a stop condition. Do not weaken or route around authorization.

Use `curl --fail-with-body --silent --show-error` and capture response bodies without verbose/header tracing when authentication is present. Keep the bearer credential out of argv and off disk: stream `Authorization: Bearer ` followed by the environment variable's raw bytes and a newline to curl's header stdin (for example, `printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD" | curl ... --header @-`). Do not place the credential in curl config syntax, which interprets backslash escapes. Parse JSON structurally rather than relying on display text.

## WorkScope admission

Coordinator API operations through scoped Bash require an active `work_scope_id` from the current snapshot. Phoenix resolves that WorkScope's server-side cwd; there is no default repository or cwd. When no active WorkScope exists, first-conversation creation is unavailable through this surface.

## Discover the default model

`GET /api/models` returns `models` and `default`. Read it before creating a conversation when the user did not name a supported model; use the `default` model identifier when present and do not guess a model ID.

## Resolve the current target

ProductConversation references accepted by the read APIs may be a product-conversation ID, transcript-row ID, or slug:

- `GET /api/product-conversations` lists aggregate identities and canonical routes.
- `GET /api/product-conversations/{reference}` returns the aggregate snapshot. Before a mutation, check `product_conversation_id`, `ordinary_lifecycle`, `latest_transcript_row_id`, and `writable_transcript_row_id`.
- `GET /api/product-conversations/{reference}/route` returns the canonical `transcript_row_id` for routing.
- `GET /api/conversations/{id}` returns transcript state, messages, `agent_working`, and `presentation_mode`.

For chat or cancel, use the snapshot's current `writable_transcript_row_id`. If it is absent, Phoenix has not exposed a writable target for those operations; do not substitute `latest_transcript_row_id`. Re-resolve immediately before acting because continuation can change the writable transcript.

Continuation is different: a context-exhausted transcript is intentionally not writable. Resolve the topology's `latest_transcript_row_id`, require `ordinary_lifecycle == "open"`, read that transcript, and continue only after `conversation.state.type == "context_exhausted"`. A History aggregate must receive a separate Open follow-up; do not call `/continue`. Do not require or target `writable_transcript_row_id` for continuation.

## Create a ProductConversation

`POST /api/product-conversations/new`

```json
{
  "request_id": "client-generated UUID",
  "cwd": "/absolute/server/path",
  "model": "supported-model-id",
  "effort": null,
  "objective": "opening user objective",
  "llm_language": null,
  "images": []
}
```

The accepted response contains `canonical_route`, `product_conversation_id`, and `transcript_row_id`. `request_id` is the creation idempotency identity: create it once and reuse the same value and payload for an uncertain exact retry. A successful response proves publication/acceptance, not completion of the opening turn. Verify with `GET /api/product-conversations/{product_conversation_id}` and then the returned/current transcript state.

Creation recovery surfaces:

- `GET /api/product-conversations/creation` returns `product_creations` and optional `next_cursor`. Follow `?cursor={next_cursor}` until the requested `request_id` is found or no cursor remains.
- `POST /api/product-conversations/creation/{request_id}/retry-delivery` retries delivery for that creation identity.
- `POST /api/product-conversations/creation/{request_id}/cancel` requests cancellation.
- `DELETE /api/product-conversations/creation/{request_id}` requests deletion where `allowed_actions` includes `delete`; it returns acceptance, then the creation worker performs cleanup.

Read `allowed_actions` first. After deletion acceptance, re-read the recovery listing until the resulting state is observed. These are creation-recovery operations, not a general conversation retry API.

## Send or steer a message

`POST /api/conversations/{writable_transcript_row_id}/chat`

```json
{
  "text": "message authorized by the user",
  "message_id": "client-generated UUID",
  "images": [],
  "files": [],
  "user_agent": null
}
```

`message_id` makes an exact chat retry idempotent. Generate it once and keep it unchanged if delivery is uncertain. The response fields are `queued`, `steering`, and `already_persisted` (false-valued optional flags may be omitted). They describe acceptance/disposition only. Verify the exact identity with:

`POST /api/conversations/{id}/messages/reconcile`

```json
{"message_ids":["the same message_id"]}
```

Each entry is `persisted`, `steering_queued`, or `absent`; the response also reports `conversation_idle`. A persisted or queued message still does not prove the assistant completed the requested work. Re-read `GET /api/conversations/{id}` and report `agent_working`/`presentation_mode` and the observed resulting messages.

## Continue a context-exhausted transcript

`POST /api/conversations/{latest_transcript_row_id}/continue`

```json
{
  "handoff": "opening continuation summary",
  "message_id": "client-generated UUID",
  "user_agent": null
}
```

Before this request, re-resolve the ProductConversation topology, verify `ordinary_lifecycle == "open"`, and verify that `GET /api/conversations/{latest_transcript_row_id}` reports `conversation.state.type == "context_exhausted"`. Reuse the same `message_id` for an exact uncertain retry. The response contains successor `conversation_id`, optional `slug`, and status `accepted`, `dispatch_failed`, or `already_exists`; `error` is present only when the successor exists but opening-message dispatch was not accepted. Preserve the returned successor identity even on `dispatch_failed`. Verify by re-reading the ProductConversation snapshot, confirming the new writable transcript, and reading the successor conversation. `accepted` and `already_exists` identify durable continuation outcomes; neither alone proves subsequent assistant execution completed.

Do not fold automatic continuation policy into this workflow. Continue only when the user authorized it.

## Cancel in-flight transcript work

`POST /api/conversations/{writable_transcript_row_id}/cancel`

The response is `{ "ok": true, "no_op": boolean }`; false `no_op` may be omitted. `no_op: true` means Phoenix observed nothing cancellable. Cancel has no caller-provided idempotency key and no uniform operation receipt. After the response, re-read the ProductConversation and transcript state to report whether work is still observed.

## Honest API gaps

Current APIs do not provide:

- a first-class Coordinator operator service or general lifecycle command endpoint;
- one ProductConversation-targeted mutation contract across create, steer, continue, retry, and cancel;
- a uniform operation ID, normalized receipt, or audit record across those actions;
- a general retry endpoint for an existing conversation turn;
- a caller idempotency key for cancel;
- proof of asynchronous execution completion in an accepted HTTP response.

Do not manufacture these guarantees with local files, ad-hoc receipt formats, database writes, polling loops, or background monitors. Use bounded follow-up reads when the user needs observation. If the requested result depends on a missing guarantee or endpoint, report the exact gap and propose a separately scoped server change.
