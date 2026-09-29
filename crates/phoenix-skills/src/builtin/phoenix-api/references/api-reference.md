# Supported Phoenix HTTP API reference

This is an authenticated operational reference for the live Phoenix deployment the user named. It is not a privileged bypass. Use the deployment's known origin; never guess that `localhost` is the intended server.

## Safe execution boundary

- Run HTTP commands only through Global's scoped Bash with an explicit active `work_scope_id` obtained from authoritative WorkScope rows. Phoenix selects that scope's server-side cwd; there is no default repository or cwd.
- Use only the public routes and fields below. Never write lifecycle state through SQLite, call internal handlers, invent a receipt, or bypass authorization.
- Prefer the live API contract. Inspect source only when `/api/version` proves the target deployment and checked-out source differ or a live response genuinely contradicts this embedded contract.

## Authorization and live capabilities

1. Call unauthenticated `GET /api/auth/status`. It returns only `auth_required` and whether this request is `authenticated`.
2. If auth is disabled, send no credential. If auth is enabled and the current request is not authenticated, use only a credential already supplied or explicitly identified by the user for this deployment. A configured password is accepted as `Authorization: Bearer …`; a `phoenix-auth` cookie is an opaque session token, not the password.
3. Keep secrets out of command arguments, tracing, files, logs, summaries, and tool output. With an already-populated `PHOENIX_PASSWORD`, stream the header through stdin: `printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD" | curl ... --header @-`. Do not print the variable or inspect databases/process environments to find a credential. Stop on `401` or `403`.
4. Authenticated `GET /api/models` is the live authority for model and effort support. Select an entry by exact `id`. Its `effort_capabilities` is one of `{"support":"unsupported"}`, `{"support":"unknown"}`, or `{"support":"supported","levels":[...],"native_default":...}`. Omit an override with JSON `null`, or send only a level listed for that exact model.

Use `curl --fail-with-body --silent --show-error`. Parse JSON structurally.

## Create one ProductConversation

`POST /api/product-conversations/new` accepts exactly:

```json
{
  "request_id": "client-generated UUID",
  "cwd": "/absolute/server/path",
  "model": "exact live model id",
  "effort": null,
  "objective": "opening user objective",
  "llm_language": null,
  "images": []
}
```

`request_id`, `cwd`, `model`, `effort`, and `objective` are immutable creation intent. Generate the UUID once. If the response is ambiguous, retain that UUID and the identical JSON payload: reconcile by `request_id`, then make only an exact retry. A changed payload with the same UUID conflicts; a new UUID risks duplicate creation.

### Copyable scoped-Bash recipe

Run the whole block in one Bash `op="run"` call with the admitted `work_scope_id`. Set `ORIGIN` to the user-selected Phoenix origin and place the exact opening objective inside the quoted heredoc. Optionally set `EFFORT` to a requested live-supported level; leaving it empty sends `null` and uses the model's native behavior.

```bash
set -u
ORIGIN='https://the-user-selected-phoenix-origin'
EFFORT='' # optional exact level; empty means JSON null
# Put the exact objective between these quoted-heredoc delimiters. Its contents
# are data, not shell syntax, so quotes, `$`, backticks, and newlines stay literal.
OBJECTIVE=$(cat <<'PHOENIX_OBJECTIVE'
the exact opening objective
PHOENIX_OBJECTIVE
) || exit

AUTH_STATUS=$(curl --fail-with-body --silent --show-error "$ORIGIN/api/auth/status") || exit
AUTH_REQUIRED=$(jq -er '.auth_required' <<<"$AUTH_STATUS") || exit
AUTHENTICATED=$(jq -er '.authenticated' <<<"$AUTH_STATUS") || exit
if [[ "$AUTH_REQUIRED" == true && "$AUTHENTICATED" != true ]]; then
  : "${PHOENIX_PASSWORD:?authenticated deployment requires a user-identified PHOENIX_PASSWORD}"
fi
api() {
  if [[ "$AUTH_REQUIRED" == true && "$AUTHENTICATED" != true ]]; then
    # Process substitution keeps the credential out of argv while leaving stdin
    # available for a streamed JSON request body.
    curl --fail-with-body --silent --show-error \
      --header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD") "$@"
  else
    curl --fail-with-body --silent --show-error "$@"
  fi
}

MODELS=$(api "$ORIGIN/api/models") || exit
MODEL=$(jq -er '.default as $d | .models[] | select(.id == $d) | .id' <<<"$MODELS") || exit
MODEL_INFO=$(jq -ec --arg model "$MODEL" '.models[] | select(.id == $model)' <<<"$MODELS") || exit
if [[ -n "$EFFORT" ]]; then
  jq -e --arg effort "$EFFORT" \
    '.effort_capabilities.support == "supported" and (.effort_capabilities.levels | index($effort) != null)' \
    <<<"$MODEL_INFO" >/dev/null || { printf '%s\n' 'requested effort is not supported by the selected live model' >&2; exit 1; }
  EFFORT_JSON=$(jq -Rn --arg value "$EFFORT" '$value')
else
  EFFORT_JSON=null
fi

CWD=$PWD
test -d "$CWD" || exit
REQUEST_ID=$(uuidgen | tr '[:upper:]' '[:lower:]') || exit
INTENT=$(jq -cn \
  --arg request_id "$REQUEST_ID" --arg cwd "$CWD" --arg model "$MODEL" \
  --argjson effort "$EFFORT_JSON" --arg objective "$OBJECTIVE" \
  '{request_id:$request_id,cwd:$cwd,model:$model,effort:$effort,objective:$objective,llm_language:null,images:[]}') || exit

# Persist the non-secret request identity in Bash output before dispatch. Keep
# REQUEST_ID and INTENT unchanged until creation is reconciled.
printf 'creation_request_id=%s\n' "$REQUEST_ID"
post_creation() {
  CREATE_WIRE=$(printf '%s' "$INTENT" | api --request POST --header 'Content-Type: application/json' \
    --data-binary @- --write-out $'\n%{http_code}' "$ORIGIN/api/product-conversations/new")
  POST_EXIT=$?
  HTTP_STATUS=${CREATE_WIRE##*$'\n'}
  CREATE=${CREATE_WIRE%$'\n'*}
}
find_creation() {
  local cursor='' page
  while :; do
    if [[ -n "$cursor" ]]; then
      page=$(api --get --data-urlencode "cursor=$cursor" "$ORIGIN/api/product-conversations/creation") || return
    else
      page=$(api "$ORIGIN/api/product-conversations/creation") || return
    fi
    RECONCILED=$(jq -ec --arg request_id "$REQUEST_ID" \
      '.product_creations[] | select(.request_id == $request_id)' <<<"$page") && return 0
    cursor=$(jq -er '.next_cursor // empty' <<<"$page") || return 1
  done
}

post_creation
if [[ $POST_EXIT -ne 0 ]]; then
  if [[ "$HTTP_STATUS" == 4* ]]; then
    printf 'creation rejected with HTTP %s; do not retry request_id=%s without correcting the request\n' "$HTTP_STATUS" "$REQUEST_ID" >&2
    exit "$POST_EXIT"
  fi
  printf 'ambiguous creation response (HTTP %s); reconciling request_id=%s\n' "$HTTP_STATUS" "$REQUEST_ID" >&2
  if find_creation; then
    jq -e --arg cwd "$CWD" --arg model "$MODEL" --arg objective "$OBJECTIVE" --argjson effort "$EFFORT_JSON" \
      '.cwd == $cwd and .model == $model and .objective == $objective and .effort == $effort' \
      <<<"$RECONCILED" >/dev/null || { printf '%s\n' 'reconciled creation intent mismatch' >&2; exit 1; }
    printf '%s\n' "$RECONCILED"
    exit 0
  fi
  printf 'creation not found after bounded pagination; retrying exact request_id=%s once\n' "$REQUEST_ID" >&2
  post_creation
  [[ $POST_EXIT -eq 0 ]] || exit "$POST_EXIT"
fi

PRODUCT_ID=$(jq -er '.product_conversation_id' <<<"$CREATE") || exit
ROOT_TRANSCRIPT=$(jq -er '.transcript_row_id' <<<"$CREATE") || exit
CANONICAL_ROUTE=$(jq -er '.canonical_route' <<<"$CREATE") || exit
SNAPSHOT=$(api "$ORIGIN/api/product-conversations/$PRODUCT_ID") || exit
CURRENT_TRANSCRIPT=$(jq -er '.latest_transcript_row_id' <<<"$SNAPSHOT") || exit
TRANSCRIPT=$(api "$ORIGIN/api/conversations/$CURRENT_TRANSCRIPT") || exit
jq -n \
  --arg request_id "$REQUEST_ID" --arg product_conversation_id "$PRODUCT_ID" \
  --arg root_transcript_row_id "$ROOT_TRANSCRIPT" --arg current_transcript_row_id "$CURRENT_TRANSCRIPT" \
  --arg canonical_route "$CANONICAL_ROUTE" \
  --arg state "$(jq -r '.conversation.state.type' <<<"$TRANSCRIPT")" \
  --argjson agent_working "$(jq '.agent_working' <<<"$TRANSCRIPT")" \
  '{request_id:$request_id,product_conversation_id:$product_conversation_id,root_transcript_row_id:$root_transcript_row_id,current_transcript_row_id:$current_transcript_row_id,canonical_route:$canonical_route,state:$state,agent_working:$agent_working}'
```

A successful POST proves durable creation/publication and returns:

- `product_conversation_id`: stable ProductConversation identity;
- `transcript_row_id`: the created root transcript row;
- `canonical_route`: the product UI route.

It does **not** prove opening-turn dispatch or model activity completed. Verify creation through `GET /api/product-conversations/{product_conversation_id}`. Then use its `latest_transcript_row_id` as the current transcript and separately read `GET /api/conversations/{current_transcript_row_id}` for `conversation.state.type`, messages, and `agent_working`. The root transcript from creation is identity/history; after continuation it can differ from the current transcript. Never substitute one meaning for the other.

### Ambiguous-response reconciliation

`GET /api/product-conversations/creation` returns `product_creations` and optional `next_cursor`. The recipe follows each cursor until the retained `request_id` is found or pagination ends, verifies `cwd`, `objective`, `model`, and `effort`, and retries the exact POST at most once only after transport/5xx ambiguity. A reconciled recovery row proves the creation request is durable but may not yet expose a published ProductConversation; report its status and use only its `allowed_actions`. Never claim absence from one page.

Creation recovery routes are scoped to that request identity:

- `POST /api/product-conversations/creation/{request_id}/retry-delivery`
- `POST /api/product-conversations/creation/{request_id}/cancel`
- `DELETE /api/product-conversations/creation/{request_id}` only when `allowed_actions` includes `delete`

Read `allowed_actions` first. These are not general conversation retry APIs. Deletion acceptance is not observable cleanup completion because pending rows leave the listing before a terminal tombstone exists.

## Resolve and act on an existing ProductConversation

Current read routes accept a ProductConversation ID, transcript-row ID, or slug as `{reference}`:

- `GET /api/product-conversations` lists stable identities and canonical routes.
- `GET /api/product-conversations/{reference}` returns the aggregate snapshot, including `ordinary_lifecycle`, `latest_transcript_row_id`, and `writable_transcript_row_id`.
- `GET /api/product-conversations/{reference}/route` returns a canonical transcript route target.
- `GET /api/conversations/{transcript_row_id}` returns exact transcript state and messages.

For message steering, use `send_conversation_message` with non-empty literal text. Do not use `POST /api/conversations/{id}/chat`: that generic browser route expands slash commands and file references. Delivered/queued is acceptance only, so read state separately when observation is required.

For cancel, re-resolve and use `POST /api/conversations/{writable_transcript_row_id}/cancel`. The response is `{ "ok": true, "no_op": boolean }`; `no_op: true` means nothing cancellable was observed. Cancel has no caller idempotency key or uniform operation receipt.

For user-authorized continuation only, require `ordinary_lifecycle == "open"` and verify `conversation.state.type == "context_exhausted"` on the exact current transcript, then call `POST /api/conversations/{latest_transcript_row_id}/continue` with a once-generated UUID `message_id`, `handoff`, and `user_agent:null`. Reuse the same `message_id` for an exact uncertain retry. Preserve the returned successor identity even for `dispatch_failed`; re-read the aggregate and successor transcript. A History aggregate requires a separate Open follow-up, not `/continue`.

## Honest gaps

Current APIs do not provide a general Coordinator lifecycle endpoint, unified mutation receipt, existing-turn retry route, cancel idempotency key, accepted-response proof of asynchronous completion, or observable completion for accepted creation-recovery deletion. Do not manufacture these with local receipt files, database writes, polling loops, background monitors, or unsupported fields/routes. Report the exact gap and scope a server change separately.
