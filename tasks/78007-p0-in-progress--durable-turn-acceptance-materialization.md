# Cut over acceptance, replay, and materialization

Depends on authoritative durable-turn repository.

Scope:
- send_chat_service and API reconciliation
- scoped idempotent acceptance with immutable prepared semantics
- canonical transcript materialization relation
- deletion/derivation of duplicate disposition and message identity authority

Acceptance:
- Exact scoped replay cannot rematerialize or change prepared semantics.
- Cross-conversation client IDs remain independent.
- One canonical message identity is authoritative.
- Existing acceptance/materialization regressions and crash matrix pass.

Out of scope: LLM/tool/steering/cancellation/runtime projection cutovers.
