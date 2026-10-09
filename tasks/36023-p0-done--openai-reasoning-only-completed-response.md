# Classify completed reasoning-only OpenAI Responses turns

Production GPT-6 Astra WebSocket requests completed with billed output tokens but no public content and were converted into synthetic HTTP 500 errors by the broad billed-empty guard. Distinguish provider-authentic completed reasoning-only quiet turns from terminal-content loss using bounded terminal shape and usage evidence. Preserve authentic tool/reasoning replay, WebSocket/SSE parity, retryable classification for ambiguous loss, and privacy-safe diagnostics.

Acceptance criteria:
- [x] Completed terminal output containing only reasoning items succeeds only when reasoning tokens account for every billed output token.
- [x] Billed empty output without complete reasoning evidence remains a retryable provider failure.
- [x] Equivalent WebSocket and SSE terminal evidence normalizes identically.
- [x] HTTP/SSE EOF before a terminal event is a retryable network interruption, not fabricated completion.
- [x] Diagnostics contain only bounded item counts/types, usage, status, and inherited correlation/transport fields.
- [x] Focused tests and clippy pass.
