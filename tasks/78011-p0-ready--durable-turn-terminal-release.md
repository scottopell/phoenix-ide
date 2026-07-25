# Cut over cancellation and terminal ownership release

Depends on LLM, tool, and steering cutovers.

Scope:
- one terminal repository command
- generation fencing, effect interruption, ownership release
- archive/delete owed-work fencing
- deletion/derivation of duplicate terminal markers

Acceptance:
- Terminal/cancel atomically fences dispatch and releases ownership.
- No terminal aggregate contains owed or claimed effects.
- Stop/cancel versus every effect completion has a deterministic typed winner.

Out of scope: final runtime/UI/SSE projections.
