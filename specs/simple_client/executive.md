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

**Progress:** 7 of 7 complete
