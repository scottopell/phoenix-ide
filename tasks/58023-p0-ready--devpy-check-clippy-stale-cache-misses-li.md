# P0: `./dev.py check` clippy serves a stale target/clippy cache, missing lints CI catches

CRITICAL WORKFLOW GAP. The local pre-push gate is unsound: `./dev.py check` can
report clippy green while CI's clippy (the same command, same pinned toolchain)
fails on the same tree. A green local check does NOT guarantee CI clippy green,
which defeats the entire purpose of the pre-push gate.

## Evidence

On PR #249, two commits passed `./dev.py check` locally (clippy reported ✓) yet
red-failed CI clippy:
- `items_after_statements` (consts after a statement in a fn body)
- `needless_raw_string_hashes` (`r#"..."#` with no interior quotes/hashes)

Both are `clippy::pedantic`, denied workspace-wide (`Cargo.toml`
`[workspace.lints.clippy] pedantic = "deny"`). CI = `./dev.py check` =
`cargo clippy -p <crate> -- -D warnings` (lane_clippy, `CARGO_TARGET_DIR=
target/clippy`), toolchain pinned 1.94.1 — identical to local. The only
difference was cache state.

## Root cause (to confirm)

The clippy lane reuses a prior result from `target/clippy` without re-linting
changed sources: locally the lane completed in ~1s (cache hit, no re-lint),
and the lints only surfaced after `rm -rf target/clippy` forced a fresh run
(~137s, which then caught them). So the clippy lane's cache key / incremental
fingerprint does not reliably invalidate on the source changes that matter,
and stale green results are served. dev.py's own comments note
`RUSTC_WORKSPACE_WRAPPER=clippy-driver` fingerprint churn and a dedicated
`target/clippy` dir to avoid clobbering the test build — that interaction is
the likely culprit (clippy-driver vs rustc fingerprint, or incremental reuse).

## Impact

- Contributors (human or agent) get false-green locally and push lint-breaking
  commits; CI ping-pongs. Observed: two wasted CI rounds on #249.
- Erodes trust in `./dev.py check` as the gate.

## Fix direction

Make the clippy lane's caching correctness-preserving:
- Ensure a source change that introduces a pedantic violation is ALWAYS caught
  by `./dev.py check` (clippy must re-lint changed crates, not serve a stale
  pass). Investigate the `target/clippy` + clippy-driver fingerprint interaction
  and incremental reuse; fix the cache key or disable incremental for the clippy
  lane if that's what's serving stale results.
- Add a verification: a deliberately-introduced pedantic violation (in a fixture
  or a scratch commit during CI of dev.py itself) must make `./dev.py check`
  fail locally — i.e., a regression guard for the gate's soundness.

## Acceptance

Introducing a `clippy::pedantic` violation into any workspace crate causes
`./dev.py check` to fail locally on a normal (non-cache-cleared) invocation,
matching CI.
