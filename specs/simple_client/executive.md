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

**Progress:** 12 of 12 complete

## Deferred: lifecycle operations

The following operations are deliberately **not** exposed by the client because they are being reworked around the transcript/compaction refactor: archive, delete, rename, cancel (conversation), upgrade-model, regenerate-name, mark-merged, and the "continued conversation" concept's lifecycle edges beyond the continuation handoff itself (which `--continue` covers against the current live contract). Task and fork-proposal approval flows (`approve-task`, `reject-task`, `task-feedback`, `abandon-task`, and the `proposals/:id/approve|dismiss|request-changes` endpoints) are likewise deferred until the lifecycle model settles. They can be added once that refactor lands.
