---
name: phoenix-api
description: Use supported Phoenix HTTP APIs for user-authorized Global Coordinator lifecycle actions.
audience: global-coordinator
---

# Phoenix API for the Global Coordinator

Use this skill only when the user has authorized a Phoenix lifecycle action. The authenticated API reference is included in this skill result; read it before acting and do not load a mutable filesystem copy.

- Use scoped Bash in an active WorkScope and the documented Phoenix HTTP API; do not invent endpoints or guarantees.
- An active local WorkScope authorizes local execution only. Separately establish the selected destination's identity, authentication, current target, and lifecycle state from authoritative, user-authorized sources. Require destination cwd provenance only when the operation consumes cwd.
- Preserve normal Phoenix authorization. When authentication requires a credential, use only a user-identified credential source bound to that destination; never serialize credentials into arguments or output, and require authenticated encryption for nonlocal transport.
- Apply the reference's existing lifecycle, target-resolution, and retry rules regardless of transport. Copyable curl recipes, local-only tools, and local cwd target only the local destination; nonlocal API calls must execute in the destination context with any required credential supplied without serialization, or stop.
- Resolve the current `ProductConversation` and its writable transcript before a transcript-targeted mutation. Continue retries stay pinned to the retained original predecessor and exact intent; otherwise do not infer a target from a stale transcript ID.
- Generate the request identity once. Reuse it for an exact retry only where the endpoint documents idempotency; never generate a fresh identity for an uncertain retry.
- Treat an HTTP success or an `accepted`/`queued` response as acceptance, not proof that asynchronous work completed. Re-read the documented state surface and report what Phoenix actually observed.
- Stop and explain the endpoint gap when the requested guarantee is unavailable. A missing endpoint requires a separately justified follow-up, not an improvised administrative workflow.
