---
name: phoenix-api
description: Use supported Phoenix HTTP APIs for user-authorized Global Coordinator lifecycle actions.
audience: global-coordinator
---

# Phoenix API for the Global Coordinator

Use this skill only when the user has authorized a Phoenix lifecycle action. The authenticated API reference is included in this skill result; read it before acting and do not load a mutable filesystem copy.

- Use scoped Bash in an active WorkScope and the documented Phoenix HTTP API; do not invent endpoints or guarantees.
- The active local WorkScope authorizes transport execution; it never substitutes for destination lifecycle authority. Same-server and cwd-dependent operations additionally require matching server identity and cwd provenance. For an existing conversation operation that does not consume destination cwd, use the reference's user-authorized SSH remote-local mode only for an authoritatively identified server and credential source.
- Remote-local mode requires verified host-key SSH, executes the documented API against destination loopback, and keeps the destination owner credential entirely in destination process memory. Never put the credential in SSH/curl arguments, output, files, or non-loopback HTTP.
- Preserve normal Phoenix authorization. Discover whether auth is enabled without exposing credentials, and never print, persist, or place credentials in command arguments or tool output.
- Resolve the current `ProductConversation` and its writable transcript before a transcript-targeted mutation. Do not infer a target from a stale transcript ID.
- Generate the request identity once. Reuse it for an exact retry only where the endpoint documents idempotency; never generate a fresh identity for an uncertain retry.
- Treat an HTTP success or an `accepted`/`queued` response as acceptance, not proof that asynchronous work completed. Re-read the documented state surface and report what Phoenix actually observed.
- Stop and explain the endpoint gap when the requested guarantee is unavailable. A missing endpoint requires a separately justified follow-up, not an improvised administrative workflow.
