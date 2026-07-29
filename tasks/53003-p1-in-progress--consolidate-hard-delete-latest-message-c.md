# Consolidate hard-delete latest-message ceiling fixture setup

Preserve exact-ceiling acceptance and over-ceiling rejection, including their public API and SSE assertions. Share only expensive conversation/message construction and loading, prove equivalent boundary-fault detection, and retain profiler evidence for targeted and warmed-lane CPU before and after.

## Evidence

Baseline: `58998d1a5caccf814ebecac370af1fe356ceb3a9` (`origin/main` when this slice began).
Implementation checkpoint: `7047a3b4ecf44644d1d3b46e789dbcf7e8762c53`.

Profiler artifacts are retained under `target/qa-evidence/53003/`:

- `direct-pair-baseline/`: immutable baseline test binary, exact two-test execution after warm-up.
- `direct-pair-after/`: immutable refactored test binary, same exact execution after warm-up.
- `before-rust-lane-58998d1a5...`: warmed baseline Rust lane.
- `after-rust-lane-7047a3b4e...`: warmed refactored Rust lane.
- `fault-existing-off-by-one.log`: original exact-ceiling test rejects a one-too-small prefix budget through the public typed ceiling error.
- `fault-refactored-off-by-one.log`: refactored exact-ceiling test catches the same typed boundary-classification fault.

The initial Cargo-command profile was rejected as the targeted metric because this crate's embedded UI build invalidated and relinked the test binary on every invocation. Immutable baseline/after test binaries isolate exactly the two test bodies while preserving the profiler's process CPU accounting.

| Scope | Before CPU | After CPU | Absolute change | Change |
|---|---:|---:|---:|---:|
| Exact accept/reject pair | 1,117.14 ms | 332.64 ms | 784.50 ms saved | 70.22% saved |
| Warmed Rust lane | 500,030.46 ms | 502,648.16 ms | 2,617.70 ms added | 0.52% slower |

Targeted wall time fell from 2,285.33 ms to 603.37 ms: 1,681.96 ms / 73.60% saved. The lane comparison is noise-dominated: host load approximately doubled between samples, and no lane-wide improvement is claimed. The fixture refactor is justified by the isolated pair result.
