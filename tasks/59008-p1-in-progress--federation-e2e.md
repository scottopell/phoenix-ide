# Implement trusted-device Phoenix federation end to end

Implement the user-authorized four bounded slices without deployment or live credential enrollment:

1. Persist stable instance identity and exclude known credential-bearing relational data from Coordinator SQL reads.
2. Add owner-managed directional enrollment, one active receiver-issued credential per caller UUID, strict authenticated HTTPS peer transport, and revocation/replacement semantics.
3. Give the Coordinator a persistent execution WorkScope that exists with zero ordinary conversations.
4. Add explicit-instance adapters for the five approved remote tools, typed destination-qualified references/provenance, Python/API/web navigation, opaque connection notes, and failure-path coverage.

Reuse existing message idempotency and destination services. Do not add native iOS switching, database synchronization, remote Bash/process management, RBAC, multitenancy, broadcast/global directory, generalized recovery, insecure TLS fallback, or production enrollment/configuration. Global retains publication, integration, deployment, and live pairing.
