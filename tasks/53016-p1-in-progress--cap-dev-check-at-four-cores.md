# Reduce development-check filesystem work and cap residual CPU

Treat elevated macOS `fseventsd` CPU as evidence that Phoenix should generate less filesystem work, not as a subscription-tuning problem. Remove redundant local check compilation and artifact namespaces first, then keep each `./dev.py check` invocation conceptually bounded to four CPU cores without shared host coordination. Measure total compiler work, files written, retained artifact size, worker counts, wall time, and host load; preserve every validation contract at its correct local or CI boundary.


## Outcome

Each check now runs one external lane command at a time and passes a saturating four-worker budget to Cargo, Rust tests, Rayon, and Vitest. This is an understandable per-invocation worker bound, not an OS CPU quota and not host-wide coordination.

Local macOS musl smoke compilation moved to the normal Linux CI Rust group, preserving pre-merge coverage once per commit while removing a 1.1 GB/3,306-file cross-target namespace from every active macOS worktree. Clippy remains in its own target because a shared-target A/B caused 1.52 GB of Clippy writes followed by 1.51 GB of Rust fingerprint repair. Its dedicated target is now non-incremental and deletes only the obsolete Clippy incremental subtree during migration: retained size fell from 2.33 GB to 731 MB, and an isolated warm A/B reduced modified bytes from 489 MB to 24 MB. A deliberate lint mutant failed, source was restored byte-for-byte, and two consecutive positive controls passed.

Disabling incremental compilation for normal Rust checks was rejected: warm writes fell from 1.40 GB to 884 MB, but wall time rose from 24.3s to 61.3s and CPU from 25.5s to 127.8s while retaining another 3.2 GB target. Worker caps and serialization also did not eliminate `fseventsd` load; a steady run still observed 62% median. Normal Rust artifact churn and retained live-worktree targets are separate follow-up concerns.

A normal compile failed with `No space left on device` at 198 MiB free, and a full check later exposed the same host-level condition while loading a Vitest suite. Task 53017 tracks explicit cleanup of generated artifacts in selected live worktrees; this task does not add automatic deletion policy. Focused unit tests, real Clippy migration, Clippy fault proof, isolated Vitest rerun, and full-check runs validate the accepted behavior; the final exact-head broad gate is recorded in the PR/CI evidence.
