# ADR-222: One authority-shaped database query surface selects local or federated execution

- **Status:** Accepted
- **Date:** 2026-11-04
- **Affects:** REQ-FED-001 through REQ-FED-003, REQ-RQD-001, REQ-RQD-005, REQ-RQD-009, REQ-RQD-010
- **Partially supersedes:** ADR-216's decision to leave remote SQL on a separate required-UUID public tool

## Context

Phoenix exposed local operator SQL as `query_database(sql)` and federated SQL as
a separate `remote_query_database(instance_id, sql)`. Other Coordinator reads use
one optional `instance` selector whose omission, explicit null, and exact
`"local"` value preserve local behavior while another string selects one saved
peer by UUID or unambiguous name. The split SQL surface forced the model to choose
a different tool and a narrower selector representation for the same semantic
operation.

Unifying the public surface must not blur authority boundaries. Ordinary writing
ProductConversations have local forensic query authority but no peer selection.
The remote branch also has stricter transport, authentication, identity,
resource, and output contracts that must remain destination-owned. A failed
explicit remote selection must never become a local query.

## Options considered

1. **Keep separate local and remote tools.** This preserves the existing shape but
   leaves redundant model-facing operations and prevents shared saved-name
   selection.
2. **Give every `query_database` registration the same optional selector.** This is
   uniform, but expands remote authority to conversations that only own local
   operator queries.
3. **Use one name with authority-specific schemas.** Coordinator
   `query_database` accepts `{sql, instance?}` while local-only registrations
   accept `{sql}`. A typed implementation variant makes the authority difference
   exhaustive.

## Decision

Choose option 3. Register only `query_database` publicly. The Coordinator variant
accepts optional `instance`; omission, explicit null, and exact `"local"` invoke
the existing local `GlobalReadService`. Any other string is a remote selector and
must resolve to exactly one saved peer by canonical UUID or unambiguous display
name before transport begins. Unknown fields, including the obsolete public
`instance_id`, fail closed. Local-only registrations retain a closed `{sql}`
schema and cannot represent remote selection.

After remote selection, Phoenix retains the existing typed peer request with
`destination_instance_id` and the complete remote-query contract for HTTPS,
peer-scoped TLS trust, authentication, caller and destination verification,
read-only SQLite authorization, credential-bearing-column denial, resource
bounds, typed cells, untrusted output, and clearability. Any remote selector,
lookup, transport, authentication, identity, admission, decoding, or execution
failure remains a remote error and never executes SQL locally.

## Consequences

- **Positive:** The Coordinator has one query operation and the same saved-peer
  selector semantics as other federated reads.
- **Positive:** The typed local/federated implementation and closed schemas make
  accidental authority expansion and silently ignored routing fields harder to
  represent.
- **Positive:** The peer endpoint and wire remain unchanged; unification does not
  weaken destination-side admission or resource controls.
- **Negative:** The same tool name has two schemas according to conversation
  authority. Registry and schema tests must preserve that distinction.
- **Negative:** Historical prompts or callers using `remote_query_database` or
  public `instance_id` fail rather than receive a compatibility alias.
- **Neutral:** Enrollment, peer trust, peer-wire identity, and the internal remote
  query helper remain unchanged.

## References

- [Federation requirements](../federation/requirements.md)
- [Remote query database requirements](../remote-query-database/requirements.md)
- [Global recall requirements](../global-recall/requirements.md)
- ADR-083: stable instance identity and known credential-query exclusions.
- ADR-216: exact local selector reuses local admission.
