# Authoritative durable turn schema and repository refinement

Depends on the durable-turn authority model phase.

Scope:
- phoenix-db schema/migrations and workflow repositories
- authoritative turn aggregate persistence and typed child effect states
- transactional commands refining the pure model
- temporary old-schema comparison as read-only projection only

Acceptance:
- One live owner per conversation is schema/repository enforced.
- Accepted nonterminal work is discoverable by one authoritative query.
- Invalid aggregate/effect combinations cannot commit.
- SQLite failpoint histories normalize to the pure model.
- Temporary shadows have explicit cutover/removal tests.

Out of scope: send-chat/runtime/tool/UI cutovers.
