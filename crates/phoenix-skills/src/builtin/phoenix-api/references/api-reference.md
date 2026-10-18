# Supported Phoenix HTTP API reference

This authenticated reference is for the live Phoenix deployment the user named. It is not a privileged bypass. Use only public routes and supported fields. Never write lifecycle state through SQLite, call internal handlers, manufacture receipts, or guess that localhost is the intended server.

## Execution and trust boundary

Every HTTP command must run through Global's Bash tool with an explicit active local `work_scope_id` admitted from authoritative Coordinator home context or WorkScope rows. That WorkScope authorizes local execution only. Separately establish the selected destination's identity, authentication, current `ProductConversation` or transcript, and lifecycle state from authoritative, user-authorized sources. Require destination cwd provenance only when the operation consumes cwd.

Transport grants no destination authority. When authentication requires a credential, use only a user-identified credential source bound to the selected destination. The copyable curl recipes are local-only: their origin and ambient `PHOENIX_PASSWORD` belong to the Phoenix server owning the active WorkScope. A nonlocal API call must instead execute in the destination context through a user-authorized, authenticated encrypted transport that supplies any required destination credential without serializing it into the Bash command; stop if that capability is unavailable. Apply the operation-specific lifecycle, target-resolution, retry, and outcome rules unchanged.

For the documented local Bash recipes, Global interprets each response structurally and retains the request identity and exact intent. Treat payloads as data, not shell syntax. Do not install dependencies or substitute ad-hoc text parsing.

These best-practice recipes are non-exhaustive, not an endpoint allowlist. Other supported public APIs may be used within the user's authority when grounded in inspected current deployment/source contracts. Inspect route registration, payload types, state guards, and response semantics as needed before acting; establish that the inspected contract applies to the selected deployment. `/api/version` can help identify the destination version, but local source alone does not prove deployed behavior. A recipe's omission is not a prohibition, and a private handler or database path is not a supported public API. Stop if support or deployment/source correspondence cannot be established. All destination authorization, transport, credential, WorkScope, immutable-target, retry, fresh-read, and server-enforcement boundaries still apply.

## Authorization and live model discovery

1. In scoped Bash, initialize the local WorkScope-owning origin from data, then call `GET /api/auth/status`:

   ```bash
   ORIGIN_B64='base64-of-the-local-workscope-origin'
   CA_CERT_PATH_B64='' # optional base64 of its authoritative CA path
   if printf '' | base64 -d >/dev/null 2>&1; then B64_DEC=(-d); else B64_DEC=(-D); fi
   ORIGIN=$(printf '%s' "$ORIGIN_B64" | base64 "${B64_DEC[@]}") || exit
   [[ "$ORIGIN" =~ ^https?://([A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:.]+\])(:[0-9]{1,5})?$ ]] || exit
   CURL_TLS=(--globoff)
   if [[ -n "$CA_CERT_PATH_B64" ]]; then
     CA_CERT_PATH=$(printf '%s' "$CA_CERT_PATH_B64" | base64 "${B64_DEC[@]}") || exit
     [[ "$CA_CERT_PATH" == /* && -r "$CA_CERT_PATH" ]] || exit
     CURL_TLS=("${CURL_TLS[@]}" --cacert "$CA_CERT_PATH")
   fi
   https_proxy= http_proxy= all_proxy= no_proxy='*' HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= NO_PROXY='*' \
     curl -q --connect-timeout 10 --max-time 30 --fail-with-body --silent --show-error "${CURL_TLS[@]}" -- "$ORIGIN/api/auth/status"
   ```

   This public route returns only `auth_required` and `authenticated`. Repeat the same `ORIGIN_B64` decode in each later scoped Bash call; shell variables do not persist across calls. For private-CA HTTPS, `CA_CERT_PATH_B64` must encode an authoritative readable CA certificate path for the local destination. Browser trust is not sufficient for server-side curl. Do not disable TLS verification; stop if the CA path is unavailable. Repeat the same origin/CA initialization in each later scoped Bash call because shell variables do not persist.
2. If auth is disabled, send no credential. If auth is required and this local request is not authenticated, use only the user-identified credential for this local deployment. A configured password is accepted as `Authorization: Bearer …`; a `phoenix-auth` cookie is an opaque session token, not the password.
3. Keep secrets out of command arguments, tracing, files, logs, summaries, and tool output. With an already-populated local `PHOENIX_PASSWORD`, stream the header through process substitution: `--header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD")`. Never print the variable or inspect databases/process environments to find a credential. Stop on `401` or `403`.
4. Call authenticated `GET /api/models`. Its `models` array and `default` field are the live authority. Select an exact model `id`: preserve a user-selected ID when present, otherwise use `default`.
5. Call authenticated `GET /api/settings/llm-language`. Its `language` is the default and `available` enumerates supported explicit values. Preserve an explicitly requested supported language on the first POST; use JSON `null` only when the user did not choose one. The selected model's `effort_capabilities` is one of `{"support":"unsupported"}`, `{"support":"unknown"}`, or `{"support":"supported","levels":[...],"native_default":...}`. Use JSON `null` for no override, or only a level listed for that exact model.

For authenticated local raw reads, repeat the same origin/CA initialization and then use:

```bash
https_proxy= http_proxy= all_proxy= no_proxy='*' HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= NO_PROXY='*' \
  curl -q --connect-timeout 10 --max-time 30 --fail-with-body --silent --show-error "${CURL_TLS[@]}" \
  --header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD") \
  -- "$ORIGIN/api/models"
```

Omit the `--header` line when auth is disabled or the request is already authenticated.

## Create one ProductConversation

This creation recipe is local-only: its origin must be the Phoenix server that owns the active WorkScope and resolves `$PWD`. For another destination, stop; never send the local executor's `$PWD` as a remote cwd.

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

Creation intent is immutable for `request_id`, `cwd`, `model`, `effort`, `objective`, and the ordered `images` collection: changing any of those under the same UUID conflicts, while a new UUID risks duplicate creation. `llm_language` is the explicit exception—the first request stores a normalized language, and every replay uses that stored value regardless of the submitted replay field. This scoped-Bash recipe supports text-only creation and therefore requires exactly `images: []`. Do not embed image data in a Bash command: API-sized image payloads exceed Bash command/output capacity. This recipe does not supply a bounded image-bearing transport; use a verified supported alternative only after inspecting the selected deployment/source contract and establishing user authority.

### 1. Generate and retain the request identity

Run this in the admitted WorkScope. Its base64 output safely represents arbitrary server paths and is non-secret durable conversation evidence; retain it before dispatch. Before generating an ID, require that trimming `$PWD` would not change it: `[[ "$PWD" != [[:space:]]* && "$PWD" != *[[:space:]] ]] || exit`. Phoenix normalizes creation cwd with `trim()`, so a leading/trailing-whitespace WorkScope path is unsupported and must stop rather than target another path.

```bash
set -u
[[ "$PWD" != [[:space:]]* && "$PWD" != *[[:space:]] ]] || exit
read -r -a UUID_BYTES <<<"$(od -An -N16 -tx1 /dev/urandom)" || exit
UUID_HEX=''
for byte in "${UUID_BYTES[@]}"; do UUID_HEX+=$byte; done
[[ "$UUID_HEX" =~ ^[0-9a-f]{32}$ ]] || exit
UUID_VARIANT=$(printf '%x' $(( (16#${UUID_HEX:16:1} & 3) | 8 ))) || exit
REQUEST_ID="${UUID_HEX:0:8}-${UUID_HEX:8:4}-4${UUID_HEX:13:3}-${UUID_VARIANT}${UUID_HEX:17:3}-${UUID_HEX:20:12}"
printf 'creation_request_id=%s\ncreation_cwd_b64=' "$REQUEST_ID"
printf '%s' "$PWD" | base64
printf '\n'
```

### 2. Prepare the exact intent as data

Global now has the retained UUID, encoded scoped cwd, exact live model/effort, explicit-or-default language selection, and a non-whitespace objective (`objective.trim()` must not be empty). Construct the complete text-only JSON object exactly once, validate that it has only the fields shown above and exactly `images: []`, and base64-encode those UTF-8 JSON bytes. Keep that base64 value unchanged through reconciliation and retry. Base64 carries arbitrary quotes, shell characters, and trailing newlines as data rather than shell syntax. This recipe supports only intents whose complete `INTENT_B64` fits comfortably inside Bash's 60k-token command-input limit. Stop this recipe for larger text objectives before UUID generation; a verified supported alternative transport requires separate contract inspection and user authority. Do not split, write, or reconstruct intent through the filesystem.

### 3. Copyable POST transport

Set only base64-alphabet literals in this scoped Bash command. `ORIGIN_B64` is the local WorkScope-owning Phoenix origin; `INTENT_B64` is the complete exact JSON from step 2. The request body is streamed on stdin, not argv. The body and HTTP status are emitted separately so Global can interpret the response structurally.

```bash
set -u
ORIGIN_B64='base64-of-the-local-workscope-origin'
CA_CERT_PATH_B64='' # optional base64 of its authoritative CA path
INTENT_B64='base64-of-the-complete-exact-json-intent'
if printf '' | base64 -d >/dev/null 2>&1; then B64_DEC=(-d); else B64_DEC=(-D); fi
ORIGIN=$(printf '%s' "$ORIGIN_B64" | base64 "${B64_DEC[@]}") || exit
[[ "$ORIGIN" =~ ^https?://([A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:.]+\])(:[0-9]{1,5})?$ ]] || exit
CURL_TLS=(--globoff)
if [[ -n "$CA_CERT_PATH_B64" ]]; then
  CA_CERT_PATH=$(printf '%s' "$CA_CERT_PATH_B64" | base64 "${B64_DEC[@]}") || exit
  [[ "$CA_CERT_PATH" == /* && -r "$CA_CERT_PATH" ]] || exit
  CURL_TLS=("${CURL_TLS[@]}" --cacert "$CA_CERT_PATH")
fi
printf '%s' "$INTENT_B64" | base64 "${B64_DEC[@]}" |
  https_proxy= http_proxy= all_proxy= no_proxy='*' HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= NO_PROXY='*' \
  curl -q --connect-timeout 10 --max-time 30 --fail-with-body --silent --show-error "${CURL_TLS[@]}" \
    --header 'Content-Type: application/json' \
    --header @<(printf '%s%s\n' 'Authorization: Bearer ' "$PHOENIX_PASSWORD") \
    --data-binary @- --write-out $'\ncreation_http_status=%{http_code}\n' \
    -- "$ORIGIN/api/product-conversations/new"
```

Omit the bearer-header line when auth is disabled or already satisfied. Keep `INTENT_B64` out of command output because it contains the objective. The 10-second connect and 30-second transfer deadlines bound every request. A POST deadline after connection may mean the server accepted the intent, so treat curl timeout as ambiguous and follow same-ID reconciliation; do not leave the handle running or mint another UUID.

HTTP 202 proves durable acceptance and returns:

- `request_id`: the exact creation identity to observe or replay;
- `product_conversation_id`: the reserved stable ProductConversation identity.

Acceptance does **not** prove publication, opening-turn dispatch, or model activity. Observe only the accepted request with `GET /api/product-conversations/creation/{request_id}`. A `published` response atomically supplies `transcript_row_id` and `canonical_route`; do not dereference or report the reserved product as published before that response.

Use bounded exact-request observation: wait between reads, stop after the caller's declared deadline, and report the last tagged status without inventing completion. The listing route `GET /api/product-conversations/creation` is supplemental discovery, not authority for an accepted request.

## Ambiguous-response reconciliation

A POST response with a semantic 4xx such as 400, 401, 403, 404, or 409 rejects that response; correct or authorize it rather than retrying unchanged. HTTP 408, 425, 429, any 5xx, or a transport failure after dispatch is ambiguous/transient: preserve the exact UUID and intent. Phoenix's auth lockout 429 currently emits no `Retry-After`; wait the observable 60-second lockout window before replay rather than retrying immediately.

For an ambiguous POST, first query `GET /api/product-conversations/creation/{request_id}`. If it returns the accepted request, continue bounded observation. If it returns 404, the original POST may still commit after the read: repeat the step-3 POST at most once with the same UUID and exact `INTENT_B64`, then resume exact-request observation. A transient replay response remains ambiguous; never release the original request identity or mint another UUID. Server idempotency returns the existing acceptance for identical intent and HTTP 409 for any changed intent, including a changed explicit `llm_language`.

Creation recovery routes are scoped to that request identity:

- `POST /api/product-conversations/creation/{request_id}/retry-delivery`
- `POST /api/product-conversations/creation/{request_id}/cancel`
- `DELETE /api/product-conversations/creation/{request_id}` only when `allowed_actions` includes `delete`

These are not general conversation retry APIs. Deletion acceptance is not observable cleanup completion because pending rows leave the listing before a terminal tombstone exists.

## Verify creation separately from activity

After exact observation returns `published`:

1. Retain the published response's `request_id`, `product_conversation_id`, root `transcript_row_id`, and `canonical_route`.
2. Read `GET /api/product-conversations/{product_conversation_id}`. When its complete JSON is observable, this independently verifies the published aggregate and returns `latest_transcript_row_id`, `writable_transcript_row_id`, lifecycle, and route data.
3. Read `GET /api/conversations/{latest_transcript_row_id}`. When complete, this separately exposes exact transcript state, messages, and `agent_working`.
4. Report `request_id`, `product_conversation_id`, root `transcript_row_id`, current `latest_transcript_row_id`, `canonical_route`, `conversation.state.type`, and `agent_working` without conflating acceptance, publication, dispatch, generation, or turn completion.

The aggregate and transcript reads can exceed Bash's result cap after large model output or message history. Validate each response as complete JSON before using it. Truncation is inconclusive, not proof that all state reads are unavailable. Use a verified supported bounded state read if its inspected deployment/source contract supplies the required identity and state evidence within user authority; bounded, deadline-limited observation is permitted. Otherwise report publication separately from unavailable activity/state verification. Never infer state from a partial body.

The root transcript returned by publication is identity/history. After continuation it can differ from the current transcript.

## Resolve and act on existing conversations

Current read routes accept a ProductConversation ID, transcript-row ID, or slug as `{reference}`:

- `GET /api/product-conversations` lists stable identities and canonical routes.
- `GET /api/product-conversations/{reference}` returns the aggregate snapshot, including `ordinary_lifecycle`, `latest_transcript_row_id`, and `writable_transcript_row_id`.
- `GET /api/product-conversations/{reference}/route` returns a canonical transcript route target.
- `GET /api/conversations/{transcript_row_id}` returns exact transcript state and messages.

For local message steering, prefer `send_conversation_message` with non-empty literal text. It resolves only on the local Phoenix instance; never substitute it for remote steering. Public chat is permitted for literal steering only through the known-receiver recipe below. Do not use omitted/default `expand_references` mode for literal steering: it expands slash commands and file references. Delivered/queued is acceptance only; read state separately when observation is required.

For cancel, re-resolve and call `POST /api/conversations/{writable_transcript_row_id}/cancel`. The response always has `ok`; `no_op` is omitted when false. Therefore `{ "ok": true }` means cancellation found in-flight work, while `{ "ok": true, "no_op": true }` means nothing cancellable was observed. Cancel has no caller idempotency key or uniform operation receipt.

Remote Continue executes that existing API in the selected destination's context and is independent of federated message steering. For user-authorized continuation only, require `ordinary_lifecycle == "open"` and verify `conversation.state.type == "context_exhausted"` on the exact current transcript. Retain that transcript as the original predecessor before calling `POST /api/conversations/{original_predecessor_id}/continue` with a once-generated UUID `message_id`, `handoff`, and `user_agent:null`. An uncertain exact retry uses the same original predecessor, message identity, and intent; never retarget a newly current successor. Preserve the successor even for `dispatch_failed`, then re-read aggregate and transcript state. A History aggregate requires a separate Open follow-up, not `/continue`.

## Public literal chat with a known receiver

This text-only recipe changes no destination authority, active WorkScope, credential rules, lifecycle admission or local-only `send_conversation_message` boundary.

1. Establish the exact user-authorized destination and normal authenticated context (`GET /api/auth/status`). Use verified HTTPS/TLS, a destination-bound CA if needed, bounded deadlines, no redirects, and credentials outside argv/output. `/api/version` is public and is not itself proof of owner authentication.
2. Fetch complete HTTP-200 JSON from `GET /api/version` on that same destination. Require full `git_sha` to exactly match an immutable commit independently verified to implement `ChatRequest`'s `literal_text` mode (or a source-inspected, accepted descendant). Inspect authoritative source at that commit; guessed semver, local capability assumptions and accepted unknown fields are not evidence. Unknown, malformed or mismatched receiver: **stop before POST**, with no expandable probe or default-policy fallback. Re-read immediately before use and each exact retry; this is not a live-replacement guarantee.
3. Freshly resolve Open and its writable transcript; read the exact transcript as complete JSON and pin its ID. On an explicitly authorized **idle** target, send one harmless `literal steering smoke test` with a retained UUID and `expansion_policy: "literal_text"`. Confirm endpoint acceptance and the materialized text bytes under that ID. Do not send slash/@/file-expandable text before known-version verification. Smoke HTTP success alone does not prove policy support: an old receiver can ignore the field.
4. Recheck step 2 immediately before POST. Send the authorized text once to `POST /api/conversations/{original_transcript_id}/chat`, retaining one UUID for this exact intent. Serialize JSON as data using the same safe stdin transport as other recipes; replace these placeholders:

   ```json
   {
     "text": "Exact user-authorized literal text",
     "message_id": "once-generated UUID retained for this exact intent",
     "expansion_policy": "literal_text",
     "images": [],
     "files": [],
     "user_agent": null
   }
   ```

   Do not use omitted/default `expand_references` mode for literal steering. The public enum is only `expand_references` or `literal_text`; internal generated context is not public. Literal mode bypasses reference expansion, not auth, attachment validation, UserApi provenance or admission. Keep attachments empty unless their destination contract is separately verified.
5. Read complete response JSON: `queued: true` is acceptance, not completion; `steering: true` queues behind running work. Observe the original transcript separately. For uncertainty, reconcile and retry only on the same original destination/transcript with identical `message_id`, policy, text, attachments and user agent after rechecking the known receiver. Never transplant to a successor, replace identity or fall back to expansion. Report actual refusals, including HTTP 409; stop on version mismatch.

## Respond to a pending structured question

This is a best-practice recipe for submitting exact user-provided answers or decisions made within explicit delegated decision scope. A generic request to inspect or unblock a conversation does not authorize choosing its answers or grant decision authority. For every question, including custom text or multiple selections, either retain the user's exact answer or establish that the user explicitly delegated this decision and make it within those limits; ask the user when neither applies. For example, “choose the test runner for this task” permits choosing a runner for that task, not approving deployment. Never fabricate approvals or extend delegation; an answer cannot grant authority beyond the user's delegation. Normal API authorization and server-enforced lifecycle/admission guards still apply; do not bypass them through private handlers or database writes.

1. Resolve the selected destination's current ProductConversation and writable transcript as above. Read `GET /api/conversations/{id}` for that exact transcript and require complete JSON with `conversation.state.type == "awaiting_user_response"`. Read `conversation.state.questions` and retain the exact opaque `conversation.state.request_id` with the destination and original transcript ID. This is the Phoenix-owned request identity, **not `tool_use_id`**; do not generate, derive, or substitute one. If the identity is absent, stop this recipe and use the normal question UI rather than inventing a legacy response.
2. Map **every exact full question text** (`question`, not `header` or an index) to the exact selected option `label` or user-authorized custom text. For `multiSelect`, the UI sends one comma-space string, e.g. `"First label, Second label"`, not a JSON array. Require complete, non-empty answers: the backend does not enforce answer completeness. Missing keys are omitted from the model-bound answer text, and unknown keys do not answer a pending question. Do not infer an answer from the option description or preview.
3. Immediately before submission, re-read the same original transcript and confirm the pending request identity and questions still match the user's authorized decision. If they changed, stop this submission; never transplant answers to a new request identity or successor transcript. Reassess the new questions separately against exact user answers or explicit delegated decision scope; ask the user if authority is missing. Retain the original identity and exact answer intent for outcome reconciliation.
4. **POST once** to `POST /api/conversations/{id}/respond` on that original transcript with JSON:

   ```json
   {
     "request_id": "exact opaque request_id from conversation.state",
     "answers": {
       "Exact full question text?": "Exact option label or user-authorized custom text"
     }
   }
   ```

   Use the same authorized destination transport, TLS verification, nonserialized credential handling, bounded request deadlines, and data-safe JSON transport as above; do not interpolate question or answer text as shell syntax. This POST body is not the creation payload and does not use a generated creation UUID. Optional `annotations` may map the same full question text to `{ "notes": "user-authorized note" }`. Omit annotations when unused. Submitted annotation `preview` is not used by the response transition; selected previews are derived from stored options, not supplied as authority by the caller.
5. Inspect the HTTP status and body, then re-read `GET /api/conversations/{id}` on the original transcript to observe the resulting state and relevant persisted answer evidence. HTTP 200 with `{ "success": true }` reports response acceptance, not completed agent execution. A changed state alone does not prove these answers were accepted. If reads are truncated or evidence is inconclusive, report the uncertainty.

The question `request_id` is a **freshness guard, not an idempotency key**: this endpoint has no caller-idempotent response receipt or replay guarantee. After a timeout, transport error, or other uncertain outcome, re-read the original destination/transcript/request identity and reconcile the original answer evidence; do not automatically POST again, even if it still appears pending. Do not replace the request ID, target a successor, or transplant answers to a new identity. HTTP 409 (`wrong_state`, `question_request_identity_required`, or `stale_question_request`) is a conflict, **not success** and not proof that an earlier submission succeeded. Freshly inspect and report the observed result; unresolved outcomes require explicit follow-up rather than a speculative retry.

Contract anchors: `respond_to_question`, `RespondToQuestionPayload`, and `validate_question_request_identity` in `api/handlers.rs`; `UserQuestionResponse` in the parent state transition; `UserQuestion`, `QuestionAnnotation`, and `QuestionRequestAuthority` in `phoenix-core`; and `QuestionPanel.buildAnswerMap` / `api.respondToQuestion` in the UI. These anchors explain the recipe, not that any particular deployment contains this source.

## Honest gaps

This is an illustrative inventory qualified by inspected deployment/source, not a denylist or a guarantee about every deployment. Potential gaps to verify include a general Coordinator lifecycle endpoint, literal-message steering on destinations without a verified known receiver, unified mutation receipts, existing-turn retry, caller-idempotent cancel, remote cwd resolution, bounded image-bearing creation transport, bounded state projections, and observable creation-recovery deletion completion. Recipe limitations do not establish that every supported alternative is absent.

Verified supported alternatives and bounded state reads remain available within the user's authority when the selected deployment's inspected contract supplies the needed semantics. Bounded, deadline-limited observation through supported reads is permitted; it does not manufacture idempotency, receipts, or completion evidence. Never invent guarantees or receipts, bypass public enforcement with database writes or private handlers, or create background monitors. Do not use local receipt files, unsupported fields/routes, or polling as substitutes for a missing server guarantee. If no verified supported path provides the requested guarantee, report the exact gap and scope a server change separately.
