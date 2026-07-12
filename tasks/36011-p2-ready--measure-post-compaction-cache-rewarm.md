# Measure post-compaction Codex cache rewarm

Build a deterministic, quota-bounded long-context fixture that reaches Phoenix's real compaction or stale-result-clearing boundary and records raw input, cached reads, cache writes, output, context usage, and semantic continuity before and after compaction. Keep prompt-token effects separate from WebSocket payload savings. Use the result to decide whether Phoenix's compaction timing should change; do not manufacture a live near-window transcript without a bounded fixture.

Acceptance criteria:
- [ ] The fixture deterministically reaches the production compaction/clearing decision path.
- [ ] Raw per-turn measurements cover the pre-boundary, compaction, first rewarm, and subsequent warm turn.
- [ ] Conversation continuity is asserted, including tool-call/result pairing.
- [ ] Any policy change is justified by measured quota impact and regression tests.
- [ ] `./dev.py check` passes.
