Implement Phoenix Wake Contracts per specs/wake-contracts/ (REQ-WAKE-001..016).

The spec is complete and reconciled (it lives on the work-scope branch /
PR #230): bash handles are now WorkScope-keyed, so every watchable handle kind
(bash, tmux, subagent) transfers across a scope-inheriting continuation, and
`Forgotten` fires only on Phoenix restart or a no-inheritor teardown — NOT at
the continuation boundary. Browser is NOT a watchable kind (no terminal state).

SCOPE (deferred — its own PR, a substantial new subsystem):
- wake_contracts SQLite table + migration + restart resync (REQ-WAKE-002)
- wake_router background poll task; on fire/expire/cancel/forget it appends a
  synthetic tool result to the conv log, triggers the next LLM turn, emits SSE
  (REQ-WAKE-003 / -006); the synthetic result is byte-shape-identical to the
  equivalent op=wait response
- is_busy() augmentation: a conv with >=1 pending contract is busy, with NO new
  conv state (stays Idle) — REQ-WAKE-004
- unified `wait_until { handle:{kind,id}, condition, max_wait_seconds }` tool,
  tagged-enum handle discriminator; v1 condition kind = HandleTerminal only
  (REQ-WAKE-005 / -016)
- terminal causes Fired/Expired/Cancelled/Forgotten + the WakeContract* SSE
  events + cost-observability metrics (REQ-WAKE-011 / -015)
- mandatory expires_at (default 600s, cap 1800s) — REQ-WAKE-007

Verify the spec is still current before starting (the bash WorkScope re-key it
depends on must have merged). Abandon the old feat/wake-contracts-spec branch —
this branch's reconciled spec supersedes it.
EOF2
