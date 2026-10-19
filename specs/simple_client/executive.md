# Simple Client - Executive Summary

## Requirements Summary

The simple client is a single-file Python CLI for interacting with the Phoenix API, designed for LLM agents. It uses single-shot execution: send message, poll for completion, print response, exit. Supports creating new conversations or continuing existing ones by ID/slug. Images can be attached via command-line flags. Output is formatted with clear section delimiters for LLM comprehension. Configuration via environment variables (`PHOENIX_API_URL`, `PHOENIX_CONVERSATION`) with command-line flag overrides.

## Technical Summary

Single Python file with PEP 723 inline dependencies (httpx, click), runnable via `uv run`. CLI accepts message as argument with options for conversation, directory, images, and API URL. Polls conversation endpoint at configurable interval until state is idle or error. Image files are read and base64-encoded before sending. Output formatted with `=== USER ===`, `=== AGENT ===`, `--- TOOL USE ---`, `--- TOOL RESULT ---` delimiters.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-CLI-001:** Single-Shot Execution | ✅ Complete | Send, poll, print, exit |
| **REQ-CLI-002:** Conversation Management | ✅ Complete | Create new or continue by ID/slug |
| **REQ-CLI-003:** Image Support | ✅ Complete | Base64 encoding with media type |
| **REQ-CLI-004:** Output Format | ✅ Complete | === USER ===, === AGENT ===, --- TOOL --- |
| **REQ-CLI-005:** Polling Behavior | ✅ Complete | Polls until idle/error with timeout |
| **REQ-CLI-006:** Configuration | ✅ Complete | PHOENIX_API_URL, --api-url, -c, -d |
| **REQ-CLI-007:** Single File Distribution | ✅ Complete | PEP 723 inline deps, uv run |
| **REQ-CLI-008:** Model Selection | ✅ Complete | --model for create, --list-models for discovery |
| **REQ-CLI-009:** Interaction | ✅ Complete | --respond, --dismiss-question, --dismiss-error, --cancel-steer, --continue |
| **REQ-CLI-010:** Introspection | ✅ Complete | --diff, --git-status, --usage, --system-prompt, --tasks, --proposals |
| **REQ-CLI-011:** Discovery | ✅ Complete | --list-conversations, --search-conversations |
| **REQ-CLI-012:** Platform & Config | ✅ Complete | --version, --deployment, --env, --mcp-status, --usage-overview, --trajectory-export |
| **REQ-CLI-013:** Owner-mediated peer operations | ✅ Implemented; mock verified | Typed JSON methods/CLI; local owner routes only; explicit peer selection, strict shape/identity checks, single-attempt send with caller UUID, no local fallback |
| **REQ-CLI-014:** Safe peer notes | ✅ Implemented; mock verified | Whole-text GET/PUT, empty/Unicode/whitespace preserved, 32 KiB UTF-8 limit, notes-only body |
| **REQ-CLI-015:** Remote subscriptions | ✅ Implemented; mock verified | Watch/list/sync/unwatch; saved qualified identity lookup, active UUID reuse, unconfirmed unsubscribe remains visible |
| **REQ-CLI-016:** Federation API reference | ✅ Implemented; mock verified | Local `GET /openapi.json`; selected peer via owner `POST /api/federation/openapi`; both peer identities checked |

**Progress:** 16 requirements implemented; federation coverage is focused Python transport/CLI testing, not live two-instance acceptance.

## Federation client surface and verification

The Python client never imports peer credentials or private-CA material. All peer actions use the configured owner's API and owner password, supplied through `PHOENIX_PASSWORD` or the existing hidden prompt. The password-valued `--password` option is not exposed. Peer display names resolve through credential-free summaries; unknown or duplicate names fail closed. `--instance` with a local action is rejected before dispatch.

Remote read/send targets are typed JSON objects, for example:

```sh
./phoenix-client.py --instance laptop --read-conversation '{"kind":"transcript","transcript_id":"opaque-id"}' --cursor 0
./phoenix-client.py --instance laptop --send-conversation-message '{"kind":"product_conversation","product_conversation_id":"product-id"}' --message-id 12345678-1234-4234-8234-123456789abc 'hello'
./phoenix-client.py --instance laptop --set-peer-notes 'Preferred build host'
./phoenix-client.py --list-remote-subscriptions
./phoenix-client.py --sync-remote-subscription 12345678-1234-4234-8234-123456789abc
./phoenix-client.py --instance laptop --openapi
```

Sync/unwatch select one saved subscription by UUID and derive its peer/product identity from that row. A UUID shared by multiple peers is rejected as ambiguous. These ID-based actions and registry listing do not accept `--instance`; the watch action does. Remote query/search/read/resolve/send/notes/watch require it. Local conversation search and local single-shot send retain their separate behavior and output.

The API reference is the federation subset, not the complete Phoenix API. Selected-peer retrieval uses the owner's facade and does not fetch URLs supplied in a document. Returned filesystem paths are JSON string handles only; Python does not test existence, expand home directories, or reveal paths.

`tests/test_phoenix_client.py` uses `httpx.MockTransport`, explicit request/body checks, and Click CLI mocks to cover owner-only routing, authentication headers, peer selection, notes boundaries, typed query cells, qualified read/search/resolve data, opaque paths, receipt discriminants, UUID preservation after uncertain send, subscription qualification, OpenAPI identity checks, malformed responses, conflict validation, and static error diagnostics. Targeted invocation:

```sh
uv run --with httpx --with httpx-sse --with click python -m unittest discover -s tests -p test_phoenix_client.py
```

## Deferred: lifecycle operations

The following operations are deliberately **not** exposed by the client because they are being reworked around the transcript/compaction refactor: archive, delete, rename, cancel (conversation), upgrade-model, regenerate-name, mark-merged, and the "continued conversation" concept's lifecycle edges beyond the continuation handoff itself (which `--continue` covers against the current live contract). Task and fork-proposal approval flows (`approve-task`, `reject-task`, `task-feedback`, `abandon-task`, and the `proposals/:id/approve|dismiss|request-changes` endpoints) are likewise deferred until the lifecycle model settles. They can be added once that refactor lands.
