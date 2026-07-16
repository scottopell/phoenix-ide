# Retain kache single-flight and optimize cold artifact hashing

Preserve the measured CPU reduction from daemon hash single-flight, then test whether hashes computed while storing fresh compiler outputs can seed the file-hash cache and avoid downstream re-reads during cold Phoenix builds. Retain correctness-preserving changes with CPU or wall-time evidence as upstream-ready patches.

## Results

### Retained: cold output hash reuse

`Store::put` already reads and BLAKE3-hashes every compiler output before storing it. The fork now records the output's fresh filesystem fingerprint with that known digest. Downstream rustc invocations no longer reread newly produced `.rlib`, `.rmeta`, and proc-macro artifacts only to rediscover the same hash. Any later file mutation changes the fingerprint and fails closed to a normal rehash.

Three interleaved fresh-cache cold builds produced:

| Metric | Baseline median | Patched median | Improvement |
|---|---:|---:|---:|
| wall time | 50.437s | 37.957s | 24.7% |
| aggregate key time | 35.155s | 30.891s | 12.1% |
| bytes hashed | 239.520 MB | 129.628 MB | 45.9% |

Wall time had substantial host noise, but every patched run deterministically hashed 129.628 MB versus 239.520 MB for every baseline run. The upstream-ready patch is `patches/kache/0003-reuse-fresh-output-hashes.patch`.

### Retained: daemon hash single-flight

Concurrent daemon requests for the same file fingerprint now share one physical hash operation and result. The earlier Phoenix benchmark removed about 8 MB of duplicate cold reads with neutral wall time (24.333s baseline, 24.328s patched). This is retained for lower CPU and memory-bandwidth pressure under parallel builds. The upstream-ready patch is `patches/kache/0004-coalesce-concurrent-file-hashing.patch`.

Store tests, daemon hash tests, and clippy pass with the combined patch stack.

### Remaining work classification

A confirmed wrapped Phoenix cold build populated the persistent hash index with approximately:

| Input class | Indexed bytes |
|---|---:|
| Rust dependency artifacts | 161.9 MB |
| Native artifacts | 63.2 MB |
| Source/configuration | 12.6 MB |
| Other/build output | 0.7 MB |

The largest remaining inputs are newly produced `.rlib`, `.rmeta`, proc-macro dynamic libraries, and an 8.3 MB native aws-lc archive. Fresh-output seeding addresses outputs kache stores. Native libraries produced by Cargo build scripts remain a promising target because `RUSTC_WRAPPER` alone does not wrap their C/C++ compiler process. A follow-up should evaluate kache as a Cargo-aware C/C++ compiler launcher before changing Phoenix defaults.
