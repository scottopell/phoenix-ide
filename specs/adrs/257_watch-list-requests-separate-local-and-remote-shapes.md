# ADR-257: Watch listing separates local and remote request shapes

- **Status:** Accepted
- **Date:** 2026-11-15
- **Affects:** REQ-FED-009

## Context

Production Ember supplies the remote-only after cursor to local watch listing and retries with different UUIDs after an empty-struct decoding error. The common schema admits that shape; only prose explains the route restriction. The user requests a structural hotfix. Other native tools already use nested operation unions to retain provider-compatible object roots.

## Options considered

1. Strengthen descriptions and the error while retaining the ambiguous shared field set.
2. Accept or silently ignore a cursor on local requests, masking an unsupported operation.
3. Use a closed request with distinct local and remote operation branches.

## Decision

Choose option 3. Local requests contain only their operation. Remote requests require a non-local saved-peer selector and optionally a nullable UUID cursor. The runtime decodes that sum type before peer resolution or listing. Keep the existing peer identity resolution, local output, remote pagination, byte budget and no-fallback behavior. Invalid input returns both first-page shapes so the model can recover.

## Consequences

- Local-plus-cursor is rejected by the advertised schema as well as the decoder.
- The object root follows the existing provider constraint; only its request property contains the union.
- Flat historical call shapes require correction rather than a compatibility parser. No database migration, cross-version replay guarantee or deployment is included.
- The tool-specific adapter bypasses the generic flat instance envelope, while reusing its exact peer resolver. Other tools retain their input contracts.

## References

- `specs/federation/requirements.md`
- `ListWatchRequest`, `ListWatchEnvelope`, `parse_list_watch_request`
- `ListWatchedConversations`, `resolve_tool_peer_allow_self`
