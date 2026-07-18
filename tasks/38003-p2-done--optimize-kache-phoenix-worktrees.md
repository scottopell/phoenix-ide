# Optimize kache for Phoenix Rust worktrees

Profile and improve the local x-kache fork for parallel, cross-worktree Cargo builds. Keep only correctness-preserving changes with repeatable Phoenix benchmark wins, add upstream-quality tests, and wire the resulting local binary into the Phoenix dev workflow without changing the default unless evidence supports it.

## Results

The dominant Phoenix-specific failure was operational, not SQLite lookup time. An isolated `KACHE_CACHE_DIR` under Phoenix's long worktree path made `daemon.sock` exceed macOS's Unix-socket path limit. The daemon exited before becoming ready, and every wrapper process fell back to local input hashing.

A controlled `cargo check -p phoenix-core --locked` comparison with fresh source/target paths showed:

| Mode | Cold | Warm 1 | Warm 2 |
|---|---:|---:|---:|
| daemon offline | 68.694s | 28.522s | 22.905s |
| daemon online | 33.206s | 25.164s | 22.717s |

Starting the daemon cut cold cache population by about 52% and improved the two cross-path warm samples by about 7% at the median. Key generation remained the dominant hit cost because relocated artifacts have new paths, inodes, and timestamps and must be read to prove content identity.

The local fork branch `phoenix-worktree-performance` contains commit `aed4a67` adding the operational `KACHE_SOCKET_PATH` override. The commit is preserved for review/upstreaming as `patches/kache/0001-configure-daemon-socket-path.patch` because `x-kache` is a separate local Git checkout.

Phoenix `dev.py` now:

- accepts `PHOENIX_KACHE_BIN` for a locally built fork;
- starts the kache daemon before launching parallel Cargo work;
- derives a short, stable `/tmp/kache-<digest>.sock` when `KACHE_CACHE_DIR` is set;
- fails clearly when daemon startup fails rather than silently benchmarking the slower fallback.

The full Phoenix Rust lane passed through the fork and active short socket: Rust test compilation, codegen, tests, and musl checking all succeeded. A speculative streaming file-hash rewrite and SQLite schema fast path were rejected because measurements did not demonstrate an end-to-end improvement.
