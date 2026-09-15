---
name: phoenix-api
description: Use supported Phoenix HTTP APIs for user-authorized Global Coordinator lifecycle actions.
audience: global-coordinator
---

# Phoenix API for the Global Coordinator

Use this skill only when the user has authorized a Phoenix lifecycle action. Read `references/api-reference.md` before acting.

- Use scoped Bash in an active WorkScope and the documented Phoenix HTTP API; do not invent endpoints or guarantees.
- Preserve normal Phoenix authorization. Discover whether auth is enabled without exposing credentials, and never print, persist, or place credentials in command arguments or tool output.
- Resolve the current `ProductConversation` and its writable transcript before a transcript-targeted mutation. Do not infer a target from a stale transcript ID.
- Generate the request identity once. Reuse it for an exact retry only where the endpoint documents idempotency; never generate a fresh identity for an uncertain retry.
- Treat an HTTP success or an `accepted`/`queued` response as acceptance, not proof that asynchronous work completed. Re-read the documented state surface and report what Phoenix actually observed.
- Stop and explain the endpoint gap when the requested guarantee is unavailable. A missing endpoint requires a separately justified follow-up, not an improvised administrative workflow.
