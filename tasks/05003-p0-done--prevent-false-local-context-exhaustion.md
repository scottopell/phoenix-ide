# Prevent false local context exhaustion

Phoenix must not terminalize a conversation from a conservative character-based prompt estimate before provider I/O. The estimator currently makes production Codex conversations unusable despite authoritative provider usage remaining below the effective context window.

## Acceptance criteria
- Requests still reach the configured provider when only the conservative estimate exceeds the route window.
- Genuine provider context-window errors still enter the existing context-exhausted path.
- Output headroom remains explicit in request telemetry and provider request limits.
- Production coordinator and p0-compaction-recovery-fix-3 can be safely retried after deploy.
