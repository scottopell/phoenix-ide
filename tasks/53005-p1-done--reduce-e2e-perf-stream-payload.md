# Reduce E2E perf_stream payload cost

Preserve the E2E streaming semantics and assertions while reducing fixture payload volume. Measure the scenario before changes, prove equivalent fault detection, and keep this as a separate stacked PR after the merge-ready browser lifecycle slice.

## Evidence

Baseline parent: `9e409c1e54ffafdeb64be29b2777a00f455a6cb4` (merge-ready browser lifecycle branch).
Final reviewed checkpoint: current branch head.

Profiler artifacts are retained under `target/qa-evidence/53005/`:

- `before-perf-stream-d280d0d40...`: 200-word scenario-only profile.
- `after-perf-stream-d280d0d40...`: 100-word scenario-only candidate profile.
- `before-e2e-lane-9e409c1e5...` / `after-e2e-lane-2ac95047b...`: warmed E2E lane profiles.
- `fault-persisted-final-word.log`: same-length persisted-content corruption was caught by a temporary lexical oracle; review correctly rejected that oracle as brittle.
- `fault-persisted-truncation.log`: completed/persisted response truncation was caught by the retained exact-count oracle.
- `correct-perf-stream.log`: the restored 100-word positive control passed.

The retained behavior-focused fault class is completed/persisted response truncation: emitting the full 100-word stream cadence but persisting 99 words reaches idle and fails the exact-count assertion with `expected 100 words from perf stream, got 99`. The scenario waits for idle plus a persisted assistant response before applying that assertion, keeping finalization synchronization separate from payload validation. A stream-accumulator-only truncation mutant was rejected as invalid evidence because the runtime correctly persists the completed response body.

| Scope | Before | After | Absolute saving | Saving |
|---|---:|---:|---:|---:|
| perf_stream wall | 4,986.69 ms | 2,632.42 ms | 2,354.28 ms | 47.21% |
| perf_stream harness CPU | 647.02 ms | 429.87 ms | 217.15 ms | 33.56% |
| perf_stream server CPU | 4.87 ms | 3.58 ms | 1.29 ms | 26.42% |
| Warmed E2E lane CPU | 15,467.66 ms | 14,863.84 ms | 603.82 ms | 3.90% |
| Warmed E2E lane wall | 34,186.87 ms | 32,385.52 ms | 1,801.35 ms | 5.27% |

The mock has no branch or finalization threshold at 200. The final 100-word fixture still emits about 100 delayed chunks, versus 62 words for the longest ordinary text scenario and 8 for the dedicated short streaming scenario.
