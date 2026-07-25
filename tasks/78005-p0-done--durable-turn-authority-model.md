# Durable turn authority model and refinement harness

Define the normative one-authority-per-semantic-fact contract for top-level durable turns and add the pure transition/refinement test foundation.

Scope:
- specs/durable-workflows/: requirements, Allium lifecycle model, executive mapping
- specs/adrs/: authority/projection and strangler migration decision
- phoenix-workflow: pure durable-turn aggregate, commands, outcomes, child effect lifecycle
- phoenix-db: deterministic failpoint/refinement harness foundation

Acceptance:
- Authority/projection/deletion map is normative and mechanically testable.
- Pure model enforces one live owner, immutable prepared semantics, typed effect lifecycle, atomic terminal fencing, scoped exact replay.
- Property tests cover generated command histories and deterministic crash points without sleeps.
- Existing regression corpus is classified against matrix cells.
- ./dev.py check passes.

Out of scope: production repository cutover and runtime/UI projection migration, tracked in dependent phases.
