# ADR-083: Federation establishes stable instance identity and excludes known credentials from agent SQL

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** REQ-GR-011A; federation instance identity

## Context

A Phoenix installation needs a durable identity before it can participate in trusted-device federation. URLs, hostnames, and display names can change, so none can be the durable key. Coordinator SQL intentionally provides broad operator-level visibility, but returning existing login, sharing, or OAuth credentials through routine agent queries would unnecessarily expose usable secrets.

## Options considered

1. Derive identity from hostname or URL: convenient, but unstable under ordinary edits and moves.
2. Persist one random UUID in each database: stable across restart and explicit about restore semantics.
3. Treat Coordinator SQL as either unrestricted or a general security sandbox: the former exposes known usable credentials, while the latter conflicts with its operator-forensics purpose and the trusted local-agent model.

## Decision

Persist exactly one random UUIDv4 per Phoenix database and expose it through a typed instance identity. Generate the UUID transactionally when its schema migration first initializes the database; reopening or rerunning migrations reads the same value.

Keep Coordinator SQL broadly readable while denying reads of known credential-bearing columns in existing owner-session, share-token, and MCP OAuth tables. Enforce the exclusions in SQLite's authorizer before row values are returned. Match by table and column rather than by generic secret-like names.

## Consequences

- Restart and reopen retain one instance identity; concurrent initialization cannot commit two identities.
- Restore behavior follows the persisted database identity and does not add a broader backup, clone, rollback, or live-replacement guarantee.
- Existing known credentials do not appear through direct selects, wildcards, aliases, views, or subqueries.
- Noncredential metadata and unrelated secret-like application data remain readable; unrestricted local Bash remains outside this bounded policy.

## References

[Global recall requirements](../global-recall/requirements.md), [compatibility requirements](../compatibility/requirements.md), ADR-034, pinned federation design input
