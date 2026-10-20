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

### Latest hosted receipt: exact `022ec5fd`

[Run 39421627851](https://github.com/scottopell/phoenix-ide/actions/runs/39421627851)
passed **205/205 focused tests in 181.846 seconds** (3384 skipped) at exact
`022ec5fd02e423614ed305f3bc97e01f30a6b36d`, using the captured full-workspace
harnesses. This includes all six private-install router tests and the typed
admission-error mapper regression. Artifact `12265740941` retains the actual
qualification evidence. Capture took 187.6 seconds; codegen reuse took 1.9
seconds and the stale check passed. Command-budget, UI/specs, and e2e passed.

Broad execution **completed**, with 5503 run, 5473 passed, 30 failed, and 191
skipped in 1207.529 seconds. All 30 failures remain unwaived; this is not an
overall-green receipt. Clippy remained red, including two owned findings now
corrected in source without suppressions: explicit remaining error variants in
the API mapper and `String::new()` in its test. Those later edits are not yet
qualified. The 205-test floor belongs only to exact `022ec5fd`; full-workspace
semantics do not establish isolated-three-package build equivalence. Default
OFF and normal-startup services-uninstalled behavior remain unchanged.
Completed-History retirement coverage remains source-only.

### Earlier checkpoint: exact `41fe2041`

Hosted [run 39409786797](https://github.com/scottopell/phoenix-ide/actions/runs/39409786797)
qualified **198/198 focused tests in 231.127 seconds** at exact commit
`41fe2041f368aab45ec319c1029e44b71412857e`, using the captured full-workspace
harnesses. Capture took 213.1 seconds; codegen reused those harnesses in 1.2
seconds and the codegen-stale check passed. Broad and focused phases consumed
the same capture without another Cargo compilation.

Artifact `12260208322` retains real `cargo.json`, `binaries.json`, `ready.json`,
and full stderr logs under
`_temp/goal-qualification-39409786797-1-check-rust/`. The receipt binds exact
HEAD/run/attempt/job, 42 executable binary identities, full-workspace/default
profile, cargo-nextest 0.9.148, Cargo 1.95.0, and rustc 1.95.0. This is a receipt
for that commit and harness/toolchain identity, not arbitrary nextest-version
compatibility or isolated-three-package build-graph equivalence.

**Overall CI remained red.** Broad execution hit its 1500-second budget:
5432/5496 tests ran, with 5399 passed, 33 failed, and 191 skipped in 1499.590
seconds. Those are partial results, not a completed broad-suite comparison or
pass. Clippy remained red. The command-budget and UI lanes also exposed three
new authored-test mistakes: missing `sys` import, an incorrect expectation of
an explicit `--workspace` flag in the unchanged broad command, and an attempt
to access the function-local `CHECK_TIMEOUT` as a module attribute. Test-only
corrections now target the actual command and local timeout/default binding;
they have not yet been executed or qualified. No helper or production behavior
change is part of those corrections.

The earlier 189-test receipt at `239afa` remains separate and scoped to its
older isolated-three-package harness; the nine historical regressions also
passed in the interrupted `39f8f3f` run. Neither receipt is silently transferred
to another commit. Mocked command-shape/failure fixtures remain distinct from
hosted metadata and OS-level process-cleanup evidence. This qualification does
not enable goal storage or authorize production activation.
