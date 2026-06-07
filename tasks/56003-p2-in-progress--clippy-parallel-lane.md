Run cargo clippy in its own parallel lane (lane_clippy) with a dedicated CARGO_TARGET_DIR=target/clippy, instead of serially in lane_rust.

WHY: warm-CI critical chain is `compile (~95s) -> rust test-run (~144s) -> clippy (~36s)`. clippy tailing the test run adds ~36s to the critical path. cargo`s exclusive workspace target lock + clippy-driver fingerprint churn (RUSTC_WORKSPACE_WRAPPER) mean a parallel clippy MUST use its own target dir to avoid (a) thrashing lane_rust/codegen workspace fingerprints and (b) serializing on the lock. sccache (PR #236) keeps the fresh dir`s dep compiles warm. Even at ~70s in its own dir, clippy hides under lane_rust (~239s) -> ~36s off the critical path.

Depends on sccache from PR #236 (stacked).

Changes (dev.py):
- run_step gains env_extra for per-step env (CARGO_TARGET_DIR override).
- new lane_clippy(); clippy removed from lane_rust.
- _LANE_INPUTS gains "clippy": {"RUST"} so it gates with rust.
- e2e target-lock docstring updated (clippy no longer contends).

Validate: ./dev.py check green; clippy runs as its own lane; confirm on CI it overlaps lane_rust.
