# Validate kache patches with native scenario harness

Run upstream kache-scenario cross-clone benchmarks against an unmodified baseline and the four-patch Phoenix optimization stack. Preserve native reports, logs, key-stability data, storage accounting, and verdicts; confirm or revise prior conclusions based on the upstream harness.

## Native harness results

A local `bench-phoenix` profile was added to the uncommitted kache checkout and executed with `kache-scenario` against Phoenix commit `4dd073c01b354cded0d877dfcd06f19b6288fe1d`. The profile uses the native clone engine: cold `cargo check -p phoenix-core --locked` in clone A, warm build in clone B at a different absolute path, one shared cache, daemon lifecycle, reports, logs, Perfetto traces, key tracing, path-leak checks, storage accounting, and verdict generation.

Both upstream baseline and the four-patch stack passed the same validity checks:

- native verdict `ok`;
- 203 hits, 0 misses, 100% weighted hit rate;
- 180/180 stable cross-clone keys;
- 218.6 MB restored entirely through APFS reflinks;
- zero wrapper errors and zero path-leak detector firings;
- identical passthrough counts and reasons.

### Fresh-run wall times

| Run | Baseline cold/warm | Patched cold/warm |
|---|---:|---:|
| first pair | 49s / 40s | 38s / 26s |
| second pair, reversed order | 34s / 25s | 36s / 24s |
| saved-cold retry | — / 25s | — / 27s |

The first pair showed a 22% cold and 35% warm wall-time improvement, but the reversed pair and retries did not reproduce the warm gap. Host/load variance dominates these short wall-clock samples. No stable warm wall-time percentage is claimed.

### Deterministic work reduction

Native `events.jsonl` from the full warm clone-B builds showed:

| Run | Baseline bytes hashed | Patched bytes hashed | Reduction |
|---|---:|---:|---:|
| first pair/retry | 285.746 MB | 171.541 MB | 40.0% |
| second pair | 291.821 MB | 171.541 MB | 41.2% |

The patched value was identical across independent runs. Aggregate key time improved from 116.494s to 73.589s in one paired retry (36.8%) and was neutral in the noisier second pair (119.886s to 118.216s).

### Conclusion

The upstream-native harness confirms that the patch stack preserves cache correctness, relocation safety, zero-copy restore, and storage behavior while removing about 41% of cross-clone input hashing. It does not yet establish a stable warm wall-time win on this host. The cold wall-time improvement is directional rather than conclusive because the second fresh pair was effectively tied.
