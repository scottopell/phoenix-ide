# ADR-047: Global Coordinator uses explicitly targeted unsandboxed Bash

- **Status:** Accepted
- **Date:** 2026-09-06
- **Affects:** REQ-BASH-012/013, REQ-GR-007; singleton Coordinator, explicit active WorkScope target
- **Supersedes:** The Global Coordinator Bash sandbox decision in [ADR-027](027_write-capable-product-conversations-use-global-evidence.md). ADR-027's global-evidence and capability-separation decisions remain in force.

## Context

The singleton Global Coordinator coordinates work across ProductConversations and WorkScopes. Its Bash capability already requires an explicit active WorkScope and resolves the selected execution directory server-side, but it reused the Explore `nono` sandbox and disappeared entirely on hosts without an enforceable Explore sandbox.

That coupled two distinct authorization decisions. Explore Bash is a read-only planning capability whose safety depends on host sandbox support. Coordinator Bash is a trusted orchestration capability whose authority comes from the singleton Coordinator identity plus an explicit, live WorkScope target. Treating sandbox availability as Coordinator authorization prevents legitimate cross-workstream coordination without strengthening WorkScope selection or process ownership.

## Options considered

### Keep Coordinator Bash sandboxed

Rejected because it couples trusted Coordinator orchestration to a host-dependent Explore restriction while adding no WorkScope authorization protection.

### Give the Coordinator generic Bash or a default cwd

Rejected because generic Bash reads its cwd from the conversation context. The Coordinator intentionally has no filesystem environment, and a default repository would make cross-WorkScope execution ambiguous and unsafe.

### Broaden Explore or ProductConversation permissions

Rejected because the requested capability belongs only to the singleton Coordinator. Reusing a broader registry or changing shared Explore policy would leak privilege beyond that boundary.

## Decision

The singleton Global Coordinator receives unsandboxed Bash through a Coordinator-only adapter to the existing write-capable Bash execution path.

Every new Coordinator command still requires an explicit active persisted WorkScope ID. Phoenix resolves and canonicalizes the selected WorkScope's execution directory server-side and rejects missing, blank, stale, invalid, or ownerless scopes before process creation. The Coordinator receives no ambient or default cwd.

The spawned process remains owned solely by the selected WorkScope. Coordinator continuation identity remains controller authorization metadata. Existing command, wait, output, process-count, handle-control, cancellation, teardown, inventory, lifecycle, health-attribution, and audit bounds remain unchanged.

Explore policy is unchanged: top-level and sub-agent Explore Bash remains sandboxed when an enforceable `nono` backend is available and absent otherwise. Direct, Work, Branch, detached work, ordinary ProductConversation, and sub-agent registries retain their existing policies and tool sets.

## Consequences

- Coordinator Bash is available independently of `nono` support.
- Coordinator commands can mutate the explicitly selected WorkScope because they are unsandboxed.
- Invalid WorkScope targeting remains fail-closed before process creation.
- Process ownership and observability remain attributed to the selected WorkScope.
- The Coordinator-only adapter remains necessary; generic no-filesystem contexts do not acquire ambient Bash authority.
- Authorization-matrix tests must prevent this capability from leaking into any other conversation or sub-agent mode.
