# Remote tool matrix — design investigation

This is an implementation-planning inventory, not an implementation admission or a general remote-tool executor contract. The five-operation matrix remains under consideration in full.

## Common transport seam

Select one enrolled instance per call; authenticate with its caller-bound token; use bounded requests/results; return destination-qualified references. Execute destination-owned service operations, not caller-supplied executable code. No broadcast, cross-host SQL joins, merged search ranking, arbitrary tool-name dispatch, background delivery queue, or automatic compatibility adapter is implied.

## Concrete matrix

| Operation | Local service anchor | Existing HTTP equivalence | Actual incremental concern |
|---|---|---|---|
| `query_database(sql)` | `GlobalReadService::query_database` → `Database::coordinator_query` | No equivalent route found in inspected registration | Expose the same validated read-only operator query on the destination. Preserve authorizer/bounds, including current privileged visibility; do not fetch raw DB files. SQL remains target-schema-specific. No schema translation. |
| `search_conversations(query)` | `GlobalReadService::search` | `/api/conversations/search` exists but uses palette search, not this tool's global message-hit policy | A thin destination adapter must use the existing tool service, preserving index-warming behavior and local Coordinator exclusion. Return source-instance-qualified citations, not rewrite formatted Markdown heuristically. No merged fleet index. |
| `read_conversation(reference, cursor)` | `GlobalReadService::read_conversation` | General conversation APIs exist; bounded agent rendering is a distinct contract | Preserve stable-current versus exact transcript/message references, page bounds, and destination-local cursor. No durable remote snapshot session implied. |
| `resolve_reference(reference)` | `GlobalReadService::resolve_reference` | `/api/global/resolve` already delegates to it | Closest to ordinary transport work. Qualify result IDs/links and keep WorkScope paths explicitly destination-host-local. Never resolve an unknown remote ID on the caller. |
| `send_conversation_message(target, message, message_id)` | `SendConversationMessage` → `SendChatApplicationService` | Ordinary chat endpoints do not carry the tool's trusted sender contract | Genuine semantic seam: enrolled source-instance attribution, server-derived sender membership, narrow remote Coordinator exception, and existing idempotent acceptance. Reuse destination admission, not a federation-owned delivery queue. |

## Assessment

The read operations are predominantly bounded adapters, not new distributed lifecycles. Their principal shared cost is instance-qualified reference production. The HTTP palette search is not interchangeable with message search merely because both are named search.

Messaging is the highest-cost operation because it crosses trusted input admission. However that cost is already required by peer Coordinator communication; including four bounded read operations need not multiply it into a general RPC platform.

Tool credential authentication identifies the calling instance. Arbitrary Bash/Python API calls remain outside conversation authorship, including creation objectives. No local UDS/peer-credential system is planned.

An optional separate instance selector on existing tools is the approved semantic interface under REQ-FED-013; endpoint wire schemas remain undecided. Source instance must remain explicit in results. Saved names are ergonomic selectors, not identity. Current-schema SQL errors are honest failures; OpenAPI cannot adapt database schemas.

## Boundaries still needing design

Typed citation/reference result representation rather than parsing current formatted strings; destination endpoint shapes; transport error codes and size/deadline values; trusted Coordinator sender identity representation; destination retention of message retry identities. These must reuse owning contracts where available rather than silently add new guarantees.
