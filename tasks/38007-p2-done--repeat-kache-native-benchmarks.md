# Repeat native kache benchmarks on a quiet host

Run three fresh interleaved upstream kache-scenario baseline/patched pairs on the pinned Phoenix workload after host activity subsides. Report cold/warm medians and ranges plus deterministic key-time and hashed-byte metrics.

## Results

Three fresh native `kache-scenario` pairs ran on a quieter host with alternating order: patched→baseline, baseline→patched, patched→baseline. Every scenario used fresh clone worktrees and cache and returned native verdict `ok` with 203 hits, zero misses, 100% key stability, zero path leaks, and full APFS reflink restore.

| Metric | Baseline | Four-patch stack | Result |
|---|---:|---:|---:|
| cold wall time | median 33s, range 33–34s | median 33s, range 33–33s | tied |
| warm wall time | median 24s, range 24–24s | median 23s, range 23–24s | about 1s / 4% faster |
| cold aggregate key time | median 106.182s | median 104.653s | 1.4% lower |
| warm aggregate key time | median 118.334s | median 118.177s | neutral |
| total key-hash bytes | median 225.855 MB, range 224.881–227.527 MB | exactly 171.541 MB in all runs | 24.1% lower |

The quiet-host series supports a modest warm wall-time improvement of about one second while confirming a deterministic 24% reduction in hashed bytes. It does not support a cold wall-time claim. Aggregate key times sum overlapping parallel wrapper durations and waiting, so they are not expected to track wall time directly.
