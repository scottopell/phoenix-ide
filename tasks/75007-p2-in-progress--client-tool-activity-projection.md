# Centralize client tool activity projection

Derive a shared pure none/executing/cancelling projection from existing ConversationState. Executing exposes the existing active tool and a readonly queued-call view; cancelling has no queue. Migrate live client consumers without changing backend, wire, persistence, scheduling, labels, timers, historical compact strips, or React memo boundaries.

Acceptance: preserve exact status labels/fallback names/queue counts; nonempty active ID validation in MessageList; exact-tool timers and final-result precedence; latest-message queued-warning suppression; reconnect and token/heartbeat/row render isolation. Validate with focused existing tests and applicable UI check lanes. No merge or deployment.
