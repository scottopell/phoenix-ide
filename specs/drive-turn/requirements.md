# drive-turn Requirements

## REQ-DRIVE-TURN-001: Production Runtime Path

WHEN an operator submits one user prompt through drive-turn
THE SYSTEM SHALL create and drive the conversation through the same Phoenix conversation runtime, state machine, LLM provider adapters, persistence layer, and built-in tool registry used by the server
AND SHALL NOT inject synthetic tool results or implement a parallel agent loop

## REQ-DRIVE-TURN-002: Stable Completion

WHEN the driven conversation reaches a stable state after processing the submitted user message
THE SYSTEM SHALL stop driving the turn
AND report the typed stable outcome

IF the conversation does not reach a stable state within the requested timeout
THEN THE SYSTEM SHALL cancel the turn through the production cancellation path
AND wait for cancellation to reach a stable state before returning
AND fail the invocation

## REQ-DRIVE-TURN-003: Database Lifetime

WHEN the operator selects memory storage
THE SYSTEM SHALL use a transient in-memory SQLite database

WHEN the operator selects temporary-file storage
THE SYSTEM SHALL use a unique SQLite database in the operating system temporary directory
AND report its retained path

WHEN the operator supplies a database path
THE SYSTEM SHALL use that SQLite file
AND report its path

THE SYSTEM SHALL reject simultaneous database-mode selections

## REQ-DRIVE-TURN-004: Structured Evidence

WHEN a driven turn completes
THE SYSTEM SHALL emit one structured JSON result containing the conversation identifier, exact build Git SHA, selected model, database lifetime, stable outcome, elapsed time, and persisted messages

## REQ-DRIVE-TURN-005: Filesystem Scope

WHEN drive-turn creates a conversation
THE SYSTEM SHALL validate the supplied working directory through the production conversation working-directory validation path
AND tools SHALL execute within that conversation scope
