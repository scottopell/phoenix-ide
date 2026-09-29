# Supported Phoenix HTTP API reference

This authenticated reference is for the live Phoenix deployment the user named. It is not a privileged bypass. Use only public routes and supported fields. Never write lifecycle state through SQLite, call internal handlers, manufacture receipts, or guess that localhost is the intended server.

## Execution and trust boundary

Every HTTP command must run through Global's Bash tool with an explicit active `work_scope_id` admitted from authoritative WorkScope rows. The selected origin must be the same Phoenix server that owns that WorkScope and resolves its scoped `$PWD`; the public API has no remote-server cwd resolver. Stop if that identity is not established.

The recipe is deliberately staged. Global structurally interprets each raw JSON response, prepares data for the next scoped Bash call, and retains the request identity and exact intent. Bash transports opaque base64 data; it does not parse JSON or interpolate user/model text into shell grammar. The only runtime commands required are `curl`, `base64`, and `od`, available on supported Phoenix hosts. Do not install dependencies or substitute ad-hoc text parsing.

Prefer the live API contract. Inspect source only when `/api/version` proves a target/source mismatch or a live response genuinely contradicts this embedded contract.

## Authorization and live model discovery

1. In scoped Bash, initialize the selected origin from data, then call `GET /api/auth/status`:

   ```bash
   ORIGIN_B64='base64-of-the-same-server-origin'
   CA_CERT_PATH_B64='' # optional base64 of authoritative same-server CA path
   ORIGIN=$(printf '%s' "$ORIGIN_B64" | base64 -d) || exit
   [[ "$ORIGIN" =~ ^https?://([A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:.]+\])(:[0-9]{1,5})?$ ]] || exit
   CURL_TLS=()
   if [[ -n "$CA_CERT_PATH_B64" ]]; then
     CA_CERT_PATH=$(printf '%s' "$CA_CERT_PATH_B64" | base64 -d) || exit
     [[ "$CA_CERT_PATH" == /* && -r "$CA_CERT_PATH" ]] || exit
     CURL_TLS=(--cacert "$CA_CERT_PATH")
   fi
   curl --fail-with-body --silent --show-error "${CURL_TLS[@]}" -- "$ORIGIN/api/auth/status"
   ```

   This public route returns only `auth_required` and `authenticated`. Repeat the same `ORIGIN_B64` decode in each later scoped Bash call; shell variables do not persist across calls. For private-CA HTTPS, `CA_CERT_PATH_B64` must encode a user-identified or authoritative same-server readable CA certificate path. Browser trust is not sufficient for server-side curl. Do not disable TLS verification; stop if the CA path is unavailable. Repeat the same origin/CA initialization in each later scoped Bash call because shell variables do not persist.
2. If auth is disabled, send no credential. If auth is required and this request is not authenticated, use only a credential already supplied or explicitly identified by the user for this deployment. A configured password is accepted as `Authorization: Bearer …`; a `phoenix-auth` cookie is an opaque session token, not the password.
3. Keep secrets out of command arguments, tracing, files, logs, summaries, and tool output. With an already-populated `PHOENIX_PASSWORD`, stream the header through process substitution: `--header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD")`. Never print the variable or inspect databases/process environments to find a credential. Stop on `401` or `403`.
4. Call authenticated `GET /api/models`. Its `models` array and `default` field are the live authority. Select an exact model `id`: preserve a user-selected ID when present, otherwise use `default`. The selected model's `effort_capabilities` is one of `{"support":"unsupported"}`, `{"support":"unknown"}`, or `{"support":"supported","levels":[...],"native_default":...}`. Use JSON `null` for no override, or only a level listed for that exact model.

For authenticated raw reads, use:

```bash
curl --fail-with-body --silent --show-error \
  --header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD") \
  -- "$ORIGIN/api/models"
```

Omit the `--header` line when auth is disabled or the request is already authenticated.

## Create one ProductConversation

`POST /api/product-conversations/new` accepts exactly:

```json
{
  "request_id": "client-generated UUID",
  "cwd": "/absolute/server-path-from-the-admitted-WorkScope",
  "model": "exact-live-model-id",
  "effort": null,
  "objective": "opening user objective",
  "llm_language": null,
  "images": []
}
```

Every request field is immutable creation intent: `request_id`, `cwd`, `model`, `effort`, `objective`, `llm_language`, and the ordered `images` collection. A changed payload with the same UUID conflicts; a new UUID risks duplicate creation. This scoped-Bash recipe supports text-only creation and therefore requires exactly `images: []`. Do not embed image data in a Bash command: API-sized image payloads exceed Bash command/output capacity. Image-bearing creation requires a separate bounded upload/transport surface that the public API does not expose.

### 1. Generate and retain the request identity

Run this in the admitted WorkScope. Its output is non-secret durable conversation evidence; retain it before dispatch.

```bash
set -u
read -r -a UUID_BYTES <<<"$(od -An -N16 -tx1 /dev/urandom)" || exit
UUID_HEX=''
for byte in "${UUID_BYTES[@]}"; do UUID_HEX+=$byte; done
[[ "$UUID_HEX" =~ ^[0-9a-f]{32}$ ]] || exit
UUID_VARIANT=$(printf '%x' $(( (16#${UUID_HEX:16:1} & 3) | 8 ))) || exit
REQUEST_ID="${UUID_HEX:0:8}-${UUID_HEX:8:4}-4${UUID_HEX:13:3}-${UUID_VARIANT}${UUID_HEX:17:3}-${UUID_HEX:20:12}"
printf 'creation_request_id=%s\ncreation_cwd=%s\n' "$REQUEST_ID" "$PWD"
```

### 2. Prepare the exact intent as data

Global now has the retained UUID, scoped `$PWD`, exact live model/effort, and objective. Construct the complete text-only JSON object exactly once, validate that it has only the fields shown above and exactly `images: []`, and base64-encode those UTF-8 JSON bytes. Keep that base64 value unchanged through reconciliation and retry. Base64 carries arbitrary quotes, shell characters, and trailing newlines as data rather than shell syntax. Reject an encoded intent that approaches the Bash tool's command limit; do not split, write, or reconstruct it through the filesystem.

### 3. Copyable POST transport

Set only base64-alphabet literals in this scoped Bash command. `ORIGIN_B64` is the same-server origin; `INTENT_B64` is the complete exact JSON from step 2. The request body is streamed on stdin, not argv. The body and HTTP status are emitted separately so Global can interpret the response structurally.

```bash
set -u
ORIGIN_B64='base64-of-the-same-server-origin'
CA_CERT_PATH_B64='' # optional authoritative same-server CA path
INTENT_B64='base64-of-the-complete-exact-json-intent'
ORIGIN=$(printf '%s' "$ORIGIN_B64" | base64 -d) || exit
[[ "$ORIGIN" =~ ^https?://[^[:space:]/]+(:[0-9]+)?$ ]] || exit
CURL_TLS=()
if [[ -n "$CA_CERT_PATH_B64" ]]; then
  CA_CERT_PATH=$(printf '%s' "$CA_CERT_PATH_B64" | base64 -d) || exit
  [[ "$CA_CERT_PATH" == /* && -r "$CA_CERT_PATH" ]] || exit
  CURL_TLS=(--cacert "$CA_CERT_PATH")
fi
printf '%s' "$INTENT_B64" | base64 -d |
  curl --fail-with-body --silent --show-error "${CURL_TLS[@]}" \
    --header 'Content-Type: application/json' \
    --header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD") \
    --data-binary @- --write-out $'\ncreation_http_status=%{http_code}\n' \
    -- "$ORIGIN/api/product-conversations/new"
```

Omit the bearer-header line when auth is disabled or already satisfied. Keep `INTENT_B64` out of command output because it contains the objective.

A successful response proves durable creation/publication and returns:

- `product_conversation_id`: stable ProductConversation identity;
- `transcript_row_id`: the created root transcript row;
- `canonical_route`: the product UI route.

It does **not** prove opening-turn dispatch or model activity completed.

## Ambiguous-response reconciliation

A definitive 4xx rejects the request; correct the request rather than retrying unchanged. A transport failure or 5xx after dispatch is ambiguous.

For ambiguity, retain the same request UUID and `INTENT_B64`, then repeat the step-3 POST at most once with those exact bytes. Server idempotency returns the existing result for the same intent or a conflict for changed intent; never mint another UUID.

`GET /api/product-conversations/creation` can supplement recovery when its complete JSON is observable. Its rows echo `cwd`, `objective`, `model`, `effort`, normalized `llm_language`, and ordered `images`; compare every field before accepting a match and follow `next_cursor` when present. However, image-bearing pages can exceed scoped Bash's output ring. Truncated or invalid JSON is inconclusive and must never be interpreted as absence. The public API has no bounded request-ID lookup, so report that observability gap rather than adding polling, filesystem output, database access, or unsafe text filtering.

Creation recovery routes are scoped to that request identity:

- `POST /api/product-conversations/creation/{request_id}/retry-delivery`
- `POST /api/product-conversations/creation/{request_id}/cancel`
- `DELETE /api/product-conversations/creation/{request_id}` only when `allowed_actions` includes `delete`

These are not general conversation retry APIs. Deletion acceptance is not observable cleanup completion because pending rows leave the listing before a terminal tombstone exists.

## Verify creation separately from activity

After a successful POST:

1. Read `GET /api/product-conversations/{product_conversation_id}`. When its complete JSON is observable, this independently verifies stable product identity/publication and returns `latest_transcript_row_id`, `writable_transcript_row_id`, lifecycle, and route data.
2. Read `GET /api/conversations/{latest_transcript_row_id}`. When complete, this separately exposes exact transcript state, messages, and `agent_working`.
3. Report `request_id`, `product_conversation_id`, root `transcript_row_id`, current `latest_transcript_row_id`, `canonical_route`, `conversation.state.type`, and `agent_working` without conflating them.

Both reads can exceed Bash's result cap after large model output or message history. Validate the response as complete JSON before using it. Truncation is inconclusive: report that creation succeeded but activity/state verification is unavailable through the current unbounded read APIs. Never infer state from a partial body.

The root transcript returned by creation is identity/history. After continuation it can differ from the current transcript. Creation success does not imply dispatch success, active generation, or turn completion.

## Resolve and act on existing conversations

Current read routes accept a ProductConversation ID, transcript-row ID, or slug as `{reference}`:

- `GET /api/product-conversations` lists stable identities and canonical routes.
- `GET /api/product-conversations/{reference}` returns the aggregate snapshot, including `ordinary_lifecycle`, `latest_transcript_row_id`, and `writable_transcript_row_id`.
- `GET /api/product-conversations/{reference}/route` returns a canonical transcript route target.
- `GET /api/conversations/{transcript_row_id}` returns exact transcript state and messages.

For message steering, use `send_conversation_message` with non-empty literal text. Do not use `POST /api/conversations/{id}/chat`: that browser route expands slash commands and file references. Delivered/queued is acceptance only; read state separately when observation is required.

For cancel, re-resolve and call `POST /api/conversations/{writable_transcript_row_id}/cancel`. The response is `{ "ok": true, "no_op": boolean }`; `no_op: true` means nothing cancellable was observed. Cancel has no caller idempotency key or uniform operation receipt.

For user-authorized continuation only, require `ordinary_lifecycle == "open"` and verify `conversation.state.type == "context_exhausted"` on the exact current transcript. Call `POST /api/conversations/{latest_transcript_row_id}/continue` with a once-generated UUID `message_id`, `handoff`, and `user_agent:null`. Reuse the same `message_id` for an uncertain exact retry. Preserve the successor even for `dispatch_failed`, then re-read aggregate and transcript state. A History aggregate requires a separate Open follow-up, not `/continue`.

## Honest gaps

Current APIs do not provide a general Coordinator lifecycle endpoint, unified mutation receipt, existing-turn retry route, cancel idempotency key, accepted-response proof of asynchronous completion, remote-server cwd resolver, bounded image upload/creation transport, bounded creation lookup by request ID, bounded state-only conversation projection, or observable completion for accepted creation-recovery deletion. Do not manufacture these with local receipt files, database writes, polling loops, background monitors, unsupported fields, or unsupported routes. Report the exact gap and scope a server change separately.
