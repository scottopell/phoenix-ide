CI run 27316908913 (pre lane-split) showed "cargo test compile" at 331s immediately after an identical 242s pre-fan-out codegen compile of the same artifacts. Review confirmed the contention theory is the best fit: at fan-out, lane_e2e's `cargo build --bin phoenix_ide` can win the shared workspace target lock (cargo acquires the build-dir lock only after config/resolve, so e2e's uv startup latency doesn't guarantee lane_rust wins), leaving lane_rust's compile as ~300s lock-wait plus a small fresh-check. The fingerprint-invalidation counter-theory is weak — nothing between the two compiles touches cargo inputs (codegen wrote only .ts files; clippy already had its own target dir).

Status of the original scope:

- CI side: mooted by the lane split (rust and e2e run on separate runners with separate target dirs) and by the in-lane codegen reorder (codegen reuses lane_rust's compiled harnesses, so the duplicate compile is gone by construction).
- Instrumentation: DONE — run_step now accumulates time between cargo's "Blocking waiting for file lock" lines and the next output line, and reports steps that spent >=1s blocked via reporter.info.
- Dedicated CARGO_TARGET_DIR for the e2e bin build: REJECTED. Locally there is no sccache, so a fresh dir means a full second dependency compile per worktree and gigabytes of disk, repaid only by seconds of lock-wait on a warm target. It is worse on the cold-worktree case it would be meant to help.

Observed: the lock-wait telemetry has now been seen in real local `./dev.py check`
runs — peak lock-wait topped out around 40s on a warm target, seen once or twice.
That is well within the benign range predicted above (a transient lock hand-off, not
the ~300s pathological contention the unsplit CI run showed), and confirms the lane
split + in-lane codegen reorder resolved the original duplicate-compile cost. Closed.
