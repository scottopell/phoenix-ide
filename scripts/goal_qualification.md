# Same-job goal qualification harnesses

`PHOENIX_GOAL_REUSE_DIR` opts the Rust check lane into a **CI-only,
full-workspace, default-nextest-profile** capture. Narrowed crate scopes,
missing nextest, and CPU-attribution/custom-profile arguments are rejected,
not silently widened or disabled. Without this opt-in, the existing check
commands and profiling behavior are unchanged.

The workflow admits reuse only for `push` and `workflow_dispatch`, whose Rust
check already selects the full workspace. PRs receive an empty opt-in and keep
the original check commands and original three-package focused Cargo command.
That selection happens before capture; failure of an admitted reuse path never
falls back to Cargo compilation. An unset, empty, or `false` opt-in is disabled.

## Accounting and semantics

Within the existing 600-second compile phase and its existing process group:

1. Run one additional, read-only `cargo metadata --locked --format-version 1`
   command with the complete dependency resolve graph (no `--no-deps`).
2. Replace `cargo nextest run --workspace --no-run` with
   `cargo nextest list --locked --workspace --list-type binaries-only
   --message-format json`. This is the compilation, not an extra build.
3. Validate both genuine JSON outputs and atomically publish a readiness receipt.

Identity validation also runs informational `rustc -vV`, `cargo -Vv`, and
`cargo nextest --version` probes before/after capture and before each consumer.
These are additional non-build commands inside the existing phase budgets.
The accounting claim is one additional **metadata graph** command and one
replacement compilation, not one additional Cargo process in total.

The existing codegen, broad, and focused phases all use `--cargo-metadata` and
`--binaries-metadata` from that capture. They do not compile again. Codegen still
selects only `export_bindings` and receives the scratch `TS_RS_EXPORT_DIR`; the
broad phase still excludes that filter. Nextest's default profile, default
filters, and ignored-test behavior remain in effect. The focused phase selects
the three goal packages **AND** the unchanged goal/historical-regression
expression. It retains two workers, immediate/final failure output, all status
lines, the seven-minute command timeout, and eight-minute step timeout. Empty
selections fail.

This qualifies **full-workspace-built harnesses**, including their workspace
feature unification. It does **not** establish build-graph equivalence to a
separate three-package Cargo compilation. A result must name the qualified
commit and this harness identity; it does not retroactively qualify another
commit or erase earlier partial/red evidence.

## Evidence custody

The private directory is derived from the actual runner temp root, run ID,
attempt, and job. Capture refuses an existing directory. JSON stdout is written
directly to its own file. Child stderr is teed, line by line and flushed, to both
the full retained stderr log and inherited stderr. This keeps the existing
check reporter's Cargo lock-wait timer observable; its merged/tail output is
not used as JSON. The artifact step retains JSON and stderr even after failure.

Children inherit the caller's process group; this helper does not change the
generic timeout implementation. That implementation is not proven to clean up
TERM-resistant descendants after their group leader exits, and no guaranteed
descendant-cleanup claim is made here.

The receipt binds clean HEAD, source root, lock/manifests, repository/ancestor/
Cargo-home configuration hashes, build-environment digest, resolved tool paths
and versions, job identity, fixed compile commands/profile, actual target paths,
complete workspace package coverage, and executable binary file identities.
Build-environment values are hashed, not dumped into artifacts. Worker/color
settings are excluded from the build digest; the cache wrapper path is
canonicalized. The CI job already sets `RUSTC_WRAPPER=sccache` for producer and
consumers, and the check lane passes its existing cache overrides to all three
phases.

Source/configuration/tool identity is checked before and after capture and again
before each consumer. Metadata hashes and executable binary device/inode/size/
mtime identities detect changes between phases. Readiness is published last.
Missing, partial, changed, or stale evidence fails without rebuilding, skipping,
repairing, or accepting another job's capture. Capture failure blocks codegen
and broad execution; the separate focused step also fails without readiness.
No artifacts are copied into a cache or reused across jobs/runs.

The JSON field contract was source-checked against nextest's
[`BinaryListSummary`, `RustBuildMetaSummary`, and `RustTestBinarySummary`](https://github.com/nextest-rs/nextest/blob/b25ca6da0e258b4341dda4e6ffcfb4317f45bde5/nextest-metadata/src/test_list.rs).
That is schema evidence, not runtime compatibility proof. The receipt records
the actual installed nextest version; unsupported output fails closed.

## Verification status

The source implementation includes command-shape and failure-path unit tests
using mocked subprocesses and temporary fixture files. These tests must be run
in an authorized qualification; authoring or static inspection is not execution
proof. The prior nine historical migration regressions passed at `39f8f3f`, but
that focused run was interrupted by its outer timeout. The earlier complete
189-test receipt at `239afa` remains scoped to that commit and its earlier
harness. Neither receipt qualifies this implementation. The first hosted
qualification must retain the real Cargo/nextest JSON and actual version receipt and demonstrate that codegen,
broad, and focused consumers used the same captured harnesses; the minimal
mocked fixtures are not nextest runtime-compatibility proof.
