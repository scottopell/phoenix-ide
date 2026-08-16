# Add actionable SQLite busy and transaction-phase telemetry

Make writer contention diagnosable without inferring lock ownership from SQLx elapsed time. Add structured database telemetry that records SQLite primary and extended result codes, operation identity, retry count, elapsed duration, and transaction phase (acquisition, statement, or commit) for critical writes and FTS maintenance.

The instrumentation must distinguish locator lookup, FTS row deletion, insert, transaction acquisition, and commit failures, avoid logging sensitive payloads, and be testable against real SQLite lock contention. Use production traces and collector warnings to validate the resulting signal before claiming a specific lock holder.
