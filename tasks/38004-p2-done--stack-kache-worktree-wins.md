# Stack further kache wins for Phoenix worktrees

Continue optimizing the local kache fork after the daemon integration win. Add correctness-preserving single-flight coordination for concurrent file hashing, measure actual duplicate work and Phoenix wall time, retain only demonstrated improvements, and preserve upstream-ready patches.

## Results

### Rejected: daemon hash single-flight

Keyed single-flight coordination reduced duplicate cold hashing by about 8 MB, but cold wall time was unchanged (24.333s baseline, 24.328s patched). Warm times were also flat. The implementation was reverted.

### Retained: seed file hashes after exact blob restore

Kache knows each cached blob's verified BLAKE3 hash. After restoring an artifact with no content/signing transformation, the fork records the restored file's fresh `(path, size, mtime, ctime, inode)` fingerprint with that known hash in the persistent file-hash cache. Downstream rustc wrappers therefore avoid reading the same restored `.rlib`/`.rmeta` solely to rediscover its digest. Transformed dep-info and signed binaries are excluded.

Six interleaved warm cross-worktree samples produced:

| Metric | Baseline median | Patched median | Improvement |
|---|---:|---:|---:|
| wall time | 21.436s | 19.905s | 7.1% |
| aggregate key time | 29.348s | 27.745s | 5.5% |
| bytes hashed | 224.881 MB | 194.962 MB | 13.3% |

The fork commit is preserved as `patches/kache/0002-reuse-restored-artifact-hashes.patch`. Store tests, wrapper tests, and clippy all pass.
