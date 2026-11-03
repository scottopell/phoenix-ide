# ADR-218: MCP redirects preserve origin and reload capture is serialized

- **Status:** Accepted
- **Date:** 2026-11-02
- **Affects:** REQ-MCP-004, REQ-MCP-015, REQ-AP-006, REQ-AP-008

## Context

Follow-up review of plugin integration found that reload captured plugin
activation before taking the reconciliation lock. A delayed enabled snapshot
could therefore re-admit a server after disable completed. The shared HTTP
transport also followed cross-origin redirects with package-configured headers,
contrary to the Agent Plugins 1.1.0 header-origin contract.

## Decision

Capture configuration files and plugin activation under the existing reload
serialization boundary, and keep that boundary through reconciliation. Startup
uses the same path. Internal reconciliation requires the held lock guard.
No second activation cache or generation counter is needed.

Use one same-origin redirect policy for all MCP HTTP transport requests, not a
plugin-only transport variant. Origin is the parsed scheme, host and effective
port. Reject cross-origin redirects and permit at most ten same-origin redirects.
The transport's shared client applies this rule to POST, GET and DELETE, including
configured generic headers and auth credentials. There is no implicit consent to
send those values to another origin.

Parse plugin endpoint URLs with the existing URL implementation and classify
literal addresses with IP loopback predicates. This includes IPv6 loopback and
the IPv4 loopback range; it does not resolve arbitrary hostnames to decide trust.

## Options considered

A plugin-only flag would preserve cross-origin behavior for other MCP servers,
but would duplicate transport policy and leave generic configured headers at
risk elsewhere. Stripping selected header names would require guessing which
values are sensitive. Rejecting cross-origin redirects keeps the authorization
boundary explicit and avoids a credential-name allowlist.

## Consequences

MCP endpoints that redirect to another origin now fail. Users must configure the
intended endpoint explicitly. Same-origin redirects remain supported. OAuth
metadata and token-endpoint clients are separate and are not changed by this
transport policy. No deployment compatibility or recovery guarantee is added.
