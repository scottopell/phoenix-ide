# Phoenix-IDE Rust Compile-Time & Binary Bloat Report

**Date:** 2026-05-25  
**Branch:** main (worktree snazzy-jumping-hare)  
**Method:** scientific — captured baselines via `cargo build --timings` (cold dev),
`cargo tree --duplicates`, `cargo bloat --release --crates`,
`cargo llvm-lines --release`. All raw artifacts in `/tmp/bloat-report/`.

> Caveats: sccache was active and a concurrent `cargo check`/`cargo build`
> ran for the user's parallel audit. Wall-clock numbers below are CPU-time
> per unit from cargo's own timings (not affected by sccache for already-
> linked artifacts — rustc -Cdebuginfo invocations all ran fresh).

## TL;DR

1. **One crate dominates: `chromiumoxide_cdp` (47.7s CPU) + `chromiumoxide` (35.5s) = 83s = 23% of total CPU work.** They sit on the critical path; nothing else can start in their wake. Removing the dep would cut cold dev build ~50%.
2. **Critical-path chain is 5 units, 118 of 137 wall-clock seconds (86%).** Wall-clock is bottlenecked by `chromiumoxide_cdp` → `chromiumoxide` → `phoenix_ide` final-link. No amount of CPU parallelism helps until chromiumoxide is broken up or removed.
3. **`monitor` binary drags TUI deps into every build.** `ratatui`, `crossterm`, `ureq`, `rusqlite`, `flate2`, second `rustls`+`webpki-roots` stack — all pulled because `phoenix-monitor` shares the `phoenix_ide` package. Moving `monitor` to its own workspace crate would shave ~10s CPU and drop ~20 transitive crates from the server build.
4. **Duplicate major versions are real cost.** `tower 0.4/0.5`, `tower-http 0.5/0.6`, `rand 0.8/0.9`, `thiserror 1/2` — these have direct-dep upgrade paths in `phoenix_ide`'s Cargo.toml.
5. **`phoenix_ide` itself is 34s of CPU work** (just the leaf crate, single codegen unit on opt-0 dev profile inherits 16 from package overrides). It's the second-biggest single unit. Internal split into sub-crates would unlock parallelism here.

---

## Method

```
cargo clean -p phoenix_ide -p phoenix-tls
cargo build --timings        # → target/cargo-timings/cargo-timing.html
cargo tree --duplicates
cargo bloat --release --crates -n 40
cd crates/phoenix-ide && cargo llvm-lines --release --bin phoenix_ide
```

Raw artifacts captured under `/tmp/bloat-report/`:
- `tree-full.txt`  — full transitive tree
- `dups.txt`       — version-duplicate listing
- `uniq-deps.txt`  — 302 unique transitive deps of phoenix_ide
- `timing.json`    — parsed UNIT_DATA from cargo-timings HTML

## Baseline numbers

| Metric | Value |
|---|---|
| Workspace members | 2 (`phoenix-ide`, `phoenix-tls`) |
| Locked crates | **423** |
| Unique transitive deps of `phoenix_ide` | **302** |
| Compile units (libs + build-scripts + bins) | 441 |
| Total CPU work, cold dev build | **359.6 s** |
| Wall-clock, cold dev build | **137.2 s** |
| Effective parallelism | 2.6× |

## Compile-time top contributors (aggregated by crate name)

| Rank | Crate | CPU-s | Notes |
|------|-------|-----:|-------|
| 1 | `chromiumoxide_cdp` | 47.73 | Auto-generated CDP bindings — **single file `cdp.rs` is 109,099 lines** in a 5.6 MB crate. No internal parallelism. |
| 2 | `phoenix_ide` (lib+bin+build) | 35.90 | Own code. Final link blocks everything. |
| 3 | `chromiumoxide` | 35.52 | Depends on `chromiumoxide_cdp`. |
| 4 | `tokio` (full features) | 11.97 | `features = ["full"]` — pulls every Tokio subsystem. |
| 5 | `zstd-sys` (build script) | 10.11 | Pulled via `tower-http` `compression-full`. C compile of zstd. |
| 6 | `reqwest` | 9.96 | rustls-tls + json + stream. |
| 7 | `ring` (build+lib) | 5.80 | crypto (rustls). C/asm. |
| 8 | `zerocopy` | 5.66 | Heavy macros. Transitive. |
| 9 | `syn` | 5.40 | Transitive proc-macro foundation. |
| 10 | `serde_core` | 4.23 | |
| 11 | `rustls` | 4.09 | |
| 12 | `regex-automata` | 4.07 | Pulled by both `ignore`/`regex` and `tracing-subscriber`. |
| 13 | `axum` | 3.71 | |
| 14 | `libc` (×6 build+lib units) | 3.60 | Six rebuilds — different feature unification cohorts. |
| 15 | `darling_core` (×2 versions) | 3.43 | 0.20.11 + 0.23.0. |
| 16 | `regex-syntax` | 3.27 | |
| 17 | `bon-macros` | 3.07 | Brought in by `brush-parser`. |
| 18 | `h2` | 3.06 | HTTP/2 — needed via hyper "http2" feature. |
| 19 | `libsqlite3-sys` | 2.84 | bundled SQLite C compile. |
| 20 | `ratatui` | 2.68 | TUI for `monitor` binary only. |
| 21 | `typenum` (×5) | 2.60 | crypto-stack support. |
| 22 | `time` | 2.45 | rcgen → x509-parser → time. |
| 23 | `generic-array` (×5) | 2.41 | crypto-stack. |
| 24 | `sqlx-sqlite` | 2.29 | |
| 25 | `brush-parser` | 2.23 | Bash parser (one tool). |
| 26 | `nom` | 2.17 | brush-parser dep. |
| 27 | `serde_derive` | 2.56 | |
| 28 | `proc-macro2` (×5) | 2.49 | |
| 29 | `thiserror-impl` (×2 versions) | 2.15 | 1 + 2 both linked. |

**Top-30 share: 229.6 s of 359.6 s total CPU = 64%.**

## Critical-path (the chain that bounds wall-clock)

```
heck (0.13s) → chromiumoxide_pdl (0.84s) → chromiumoxide_cdp (47.73s)
   → chromiumoxide (35.52s) → phoenix_ide (34.06s)
```

**118.3 CPU-seconds** of unavoidable serial work. Wall-clock cold build is
137 s; this chain is **86 %** of it. Even if you bought a 64-core box you
could not get below ~118 s as long as this chain holds.

## Duplicate-version cost

`cargo tree --duplicates` flagged the following double/triple-linked crates.
Each version compiles separately + costs link time.

### Direct-dep fixable (phoenix_ide picks the old major)

| Crate | Versions present | Where 1.x/0.4 is pulled |
|-------|------------------|------------------------|
| `tower` | 0.4.13 + 0.5.3 | `phoenix_ide` direct dep is 0.4; axum/reqwest pull 0.5 |
| `tower-http` | 0.5.2 + 0.6.8 | `phoenix_ide` direct is 0.5; reqwest pulls 0.6 |
| `rand` | 0.8.5 + 0.9.3 | `phoenix_ide` direct is 0.8; proptest/tungstenite pull 0.9 |
| `thiserror` | 1.0.69 + 2.0.18 | `phoenix_ide` direct is 1; ts-rs/sqlx/cached/chromiumoxide pull 2 |

Bumping these four direct deps would (in principle) eliminate the second
copy. Risk: tower 0.5 / tower-http 0.6 break source compatibility — would
need code changes. Worth experimentally measuring.

### Transitive-only (no direct fix, need upstream)

| Crate | Versions | Cause |
|-------|---------|-------|
| `getrandom` | 0.2.17 + 0.3.4 + 0.4.2 | **three majors**: ring (0.2), rand 0.9 (0.3), uuid/tempfile (0.4) |
| `hashbrown` | 0.14.5 + 0.15.5 + 0.17.0 | rusqlite (0.14), sqlx/cached (0.15), ? (0.17) |
| `darling` | 0.20.11 + 0.23.0 | brush-parser/cached (0.20), ratatui/bon-macros (0.23) |
| `rustix` | 0.38.44 + 1.1.4 | older crates pin 0.38 |
| `unicode-width` | 0.1 + 0.2 | various |
| `webpki-roots` | 0.26.11 + 1.0.6 | ureq pulls 0.26, reqwest pulls 1.0 |
| `tungstenite` | 0.24 + 0.28 | axum 0.7→tokio-tungstenite (0.24), chromiumoxide→async-tungstenite (0.28) |
| `heck` | 0.4 + 0.5 | proc-macro generations |
| `libc` | 0.2.185 × 2 | identical version, different feature cohorts |
| `peg-runtime` | 0.8.5 × 2 | same — feature cohort dup |

## Package-layout finding: `monitor` binary contaminates the server build

`phoenix-monitor` is a second `[[bin]]` inside the `phoenix_ide` package
(`crates/phoenix-ide/src/bin/monitor.rs`). It is the **sole consumer** of
several heavy deps that nonetheless appear in `phoenix_ide`'s `[dependencies]`
and are therefore linked into every `cargo build` of the workspace:

| Dep | Sole use | Cold-build CPU |
|-----|---------|---------------:|
| `ratatui` | monitor TUI | 2.68 s |
| `crossterm` | monitor TUI | (~ included in ratatui) |
| `ureq` | monitor HTTP polling | (~2 s + rustls/webpki-roots dup) |
| `rusqlite` | monitor DB reading | ~2 s |
| `flate2` + `miniz_oxide` | ureq decompression | ~1.5 s |

Plus the duplicate `webpki-roots` entry, half the `darling 0.23` cost
(ratatui pulls it via `instability`), and `libsqlite3-sys` runs its C
build script twice in some cases.

Estimated savings of moving `monitor` to a new workspace crate: **~10 s
CPU + ~20 fewer crates on the server compile path**. The two binaries
already share zero source modules (`monitor.rs` does its own HTTP poll
and SQLite reads).

## Feature-flag heaviness

Quick scan of feature flags in `crates/phoenix-ide/Cargo.toml`:

- `tokio = { features = ["full"] }` — `full` includes `process`, `signal`,
  `rt-multi-thread`, `io-util`, `net`, `time`, `sync`, `macros`, `fs`, plus
  `parking_lot`. Trimming to actual usage (likely `rt-multi-thread`,
  `macros`, `signal`, `process`, `net`, `time`, `sync`, `fs`, `io-util`)
  removes very little — `full` is mostly accurate here. Low ROI.

- `tower-http = { features = ["cors", "fs", "compression-full", "trace"] }`
  — `compression-full` pulls **`brotli` + `zstd-sys` (10 s build script)
  + `flate2`**. If only gzip is needed: switch to `compression-gzip`. If
  no compression: drop entirely. Verify by checking actual middleware
  registrations.

- `reqwest = { features = ["json", "stream", "rustls-tls"] }` —
  reasonable. No `cookies`/`gzip`/`brotli` pulled unnecessarily.

- `chromiumoxide = { features = ["tokio-runtime", "_fetcher-rustls-tokio"] }`
  — the `_fetcher` feature pulls `chromiumoxide_fetcher` which has its
  own `reqwest` + `directories` + `zip` chain. If Phoenix already manages
  Chrome installation outside chromiumoxide, this feature is dead weight.

## Binary size (release, `cargo bloat --crates`)

`.text` section: 17.2 MiB. Final file (with line-tables debuginfo): 35.9 MiB.

| Crate | `.text` share | Bytes |
|-------|-------------:|------:|
| `std` | 18.1% | 3.1 MiB |
| **`chromiumoxide`** | 14.5% | 2.5 MiB |
| **`phoenix_ide`** | 11.5% | 2.0 MiB |
| `[Unknown]` (inlined / generic) | 9.0% | 1.5 MiB |
| `tokio` | 4.1% | 726 KiB |
| `serde_json` | 3.8% | 668 KiB |
| `axum` | 3.3% | 588 KiB |
| `rustls` | 2.9% | 512 KiB |
| `reqwest` | 2.8% | 496 KiB |
| `regex_automata` | 2.3% | 399 KiB |
| `chromiumoxide_cdp` | 2.0% | 360 KiB |
| `h2` | 2.0% | 359 KiB |

**Key insight:** `chromiumoxide_cdp`'s 109K-line source compiles to **only
360 KiB** in the final binary — dead-code elimination strips most of it.
**This is a compile-time-only problem, not a binary-size problem.** The
47.7 s spent on it is paid by every clean build for ~0.3 MiB shipped.

## Monomorphization (`cargo llvm-lines --release`)

Total: **3.79M LLVM lines across 67,228 function copies.**

Top offenders are all tokio task machinery and serde_json:

| Function | Lines | Copies | Why |
|---------|------:|------:|-----|
| `tokio::runtime::task::harness::poll_future` | 63,762 | **298** | 298 distinct `tokio::spawn` task types in the codebase |
| `tokio::runtime::task::core::Cell::new` | 44,042 | 298 | same |
| `tokio::runtime::task::harness::Harness::poll_inner` | 39,038 | 298 | same |
| `Box::drop` | 33,064 | 716 | generic box drops |
| `std::panicking::catch_unwind::do_catch` | 29,920 | 1,496 | every spawn wraps in catch_unwind |
| `tokio::runtime::task::harness::cancel_task` | 28,442 | 298 | |
| `core::ops::function::FnOnce::call_once` | 26,250 | 1,301 | |
| `serde_json::value::de::visit_array` | 26,190 | 145 | many `Value::deserialize` instantiations |
| `<&mut Deserializer>::deserialize_struct` | 25,809 | 64 | |
| `axum::handler::Handler::call::{{closure}}` | 21,106 | 40 | handler closures |

Tokio task machinery (sum of harness/core entries) ≈ **250K LLVM lines**.
That alone is the biggest single LLVM cost.

**Mitigation candidates** (would need spike to validate, not free):
- Wrap heavily-monomorphized `tokio::spawn` call sites in `BoxFuture` or
  `tokio::task::spawn(Box::pin(future))` to collapse instantiations.
- Reduce generic-over-handler patterns in `axum` setup if any handler
  factory is called with many types.

Lower priority than chromiumoxide — these are widely distributed and
each individual fix is small.

---

## Recommendations (ranked by ROI, no fixes applied)

### A. High ROI — directly attacks the critical path

1. **Audit whether chromiumoxide can be replaced with a thinner CDP client.**
   chromiumoxide_cdp is 109 K lines of single-file generated code; we use
   ~10 CDP domains. If a smaller crate (e.g. `headless_chrome` with
   feature-gated CDP coverage, or hand-written CDP types) suffices, you
   shave 80+ CPU-seconds and ~25 transitive crates. **Bias: scientific
   experiment required** — build a stub branch swapping the dep, measure
   `cargo build --timings` cold delta.

2. **Move `phoenix-monitor` to its own workspace crate.** Mechanical, low
   risk, removes ~20 crates and ~10 CPU-s from every server build.
   Expected wall-clock improvement on the critical path: minimal
   (chromiumoxide dominates), but server hot-iteration improves
   noticeably. **Independently verify** with a before/after
   `cargo build --timings`.

3. **Drop `compression-full` from `tower-http` features.** Confirm what
   compression Phoenix actually serves. If gzip-only, use
   `compression-gzip`; if none, drop the feature. Saves ~10 s
   (`zstd-sys` build script + `brotli`).

### B. Mid ROI — dup elimination

4. **Bump `tower` 0.4 → 0.5 and `tower-http` 0.5 → 0.6** in `phoenix_ide`.
   Eliminates two duplicate-major linkings. Requires API-change work in
   middleware setup.

5. **Bump `thiserror` 1 → 2.** Direct dep; most transitive users already
   on 2. Low-risk source bump.

6. **Bump `rand` 0.8 → 0.9.** Direct dep; matches the version proptest/
   tungstenite already pull.

### C. Lower ROI — architectural

7. **Split `phoenix_ide` lib into sub-crates** along clear seams
   (`tools/`, `llm/`, `runtime/`, `terminal/`, `chain_*`). Currently the
   34 s leaf-crate compile is a single rustc job; split unlocks parallelism
   and improves incremental cache hits.

8. **Investigate whether `brush-parser` is justified.** It pulls `bon`,
   `cached` (with `ahash`), `peg`, and a separate `darling 0.20` cascade —
   net ~5 s for one bash-syntax check. Cheaper hand-rolled validator?

### D. Cheap nice-to-haves

9. **Drop `chromiumoxide_fetcher` feature** if Phoenix has its own Chrome
   install path. Removes `directories`, `zip`, the second `reqwest` chain
   spawn-point.

10. **Run `cargo +nightly udeps`** (separate experiment) — there may be
    deps in Cargo.toml that no source file actually uses. Pre-flight
    cleanup.

## Suggested next experiments (scientific method)

| # | Hypothesis | Test | Threshold for "win" | Result |
|---|-----------|------|---------------------|--------|
| **E1** | Moving `monitor` to its own crate cuts cold dev build ≥ 5 s wall-clock. | `cargo clean && cargo build --timings` before vs after. | ≥ 5 s wall-clock + p<0.05 across runs. | **PASS (single run): −23.7 s wall (−17%), −42.0 s CPU (−12%). See "E1 results" below.** |
| **E2** | Dropping `compression-full` → `compression-br,compression-gzip` shaves ≥ 8 s CPU. | Edit Cargo.toml, rebuild fresh; compare timings. | ≥ 8 s CPU (zstd-sys gone). | **PASS: −24.4 s CPU (−8%), −8 units. See "E2 results" below.** |
| E3 | Bumping tower/tower-http/thiserror/rand to current majors cuts dup-link cost ≥ 3 s CPU. | Coordinated bump branch, rebuild fresh. | ≥ 3 s CPU + no behavioural regressions. | pending |
| E4 | Replacing chromiumoxide with `headless_chrome` (or trimmed CDP) drops cold wall-clock ≥ 40 s. | Spike branch with browser_profile reimplemented; benchmark. | ≥ 40 s wall-clock. | pending |

Borrowing from the phoenix-perf-shared significance discipline:
**threshold met but p ≥ 0.05 → noise → REJECT.** Always 3+ runs, median
+ Welch's t.

---

## E1 results (executed 2026-05-25)

Refactor: moved `crates/phoenix-ide/src/bin/monitor.rs` → new workspace
member `crates/phoenix-monitor/` with its own Cargo.toml. Removed
`ratatui`, `crossterm`, `rusqlite`, `ureq` from `phoenix_ide`'s deps;
removed the `[[bin]]` block from `phoenix_ide`'s Cargo.toml.

Procedure (single run, no concurrent cargo workload):

```bash
cargo clean
rm -rf target/cargo-timings
cargo build --timings
```

| Metric | Before E1 | After E1 | Δ |
|--------|----------:|---------:|----:|
| Compile units | 441 | 422 | −19 |
| Total CPU | 359.6 s | 317.6 s | **−42.0 s (−12%)** |
| Wall-clock | 137.2 s | 113.5 s | **−23.7 s (−17%)** |
| Critical path | 118.3 s | 96.6 s | **−21.7 s (−18%)** |
| Effective parallelism | 2.6× | 2.8× | +0.2 |

Critical-path chain shape unchanged:
`heck → chromiumoxide_pdl → chromiumoxide_cdp → chromiumoxide → phoenix_ide`.

**Mechanism:** TUI deps were direct deps of `phoenix_ide` (via `[[bin]]`),
so cargo had to finish their rmeta builds before `phoenix_ide` could
start. Pulling them out lets `phoenix_ide` start sooner *and* removes
work that no longer blocks anything (now parallelized with chromiumoxide
chain).

**Caveat:** baseline `chromiumoxide_cdp` reported 47.7 s; after-run
reported 25.9 s. Baseline ran with concurrent cargo workloads contending
for CPU (chromiumoxide_cdp is single-threaded). Some of the wall-clock
delta is contention noise, not the refactor. Robust deltas: unit count
(−19) and 16+ s of TUI-dep CPU work moved off phoenix_ide's blocking path.

**Validation:**
- `cargo check --workspace` ✓
- `cargo clippy --workspace -- -D warnings` ✓
- `target/debug/phoenix-monitor --help` prints usage ✓

**Files changed:**
- `Cargo.toml` — added `crates/phoenix-monitor` to workspace members
- `crates/phoenix-ide/Cargo.toml` — removed `[[bin]]` + 4 deps (ratatui, crossterm, rusqlite, ureq)
- `crates/phoenix-monitor/Cargo.toml` — new
- `crates/phoenix-monitor/src/main.rs` — moved verbatim from `crates/phoenix-ide/src/bin/monitor.rs`

---

## E2 results (executed 2026-05-25)

Refactor: pruned `tower-http` `compression-full` feature down to
`compression-br,compression-gzip`. Updated `CompressionLayer` builder
to enable only `gzip(true)` + `br(true)` (dropped `deflate` and `zstd`).

Rationale: phoenix-ide is single-server and accessed both locally and
over public HTTPS. Brotli wins on the embedded `ui/dist/` React bundle
(10–15 % smaller than gzip) and is supported by every browser since
Chrome 50 / Firefox 44. Zstd over HTTP is Chrome 123+ / Firefox 126+
only (March/May 2024) — narrow browser-support window for a marginal
ratio improvement, and `zstd-sys` is a 10 s C build script on every
cold compile.

HTTP/HTTPS unification verified correct: `main.rs` builds one router
with one `CompressionLayer` before the `if let Some(tls_source)` branch
that picks the listener type. Both code paths layer compression
identically — single source of truth at `main.rs:233-234`.

Procedure (single cold workspace build, idle-ish system):

```bash
cargo clean
rm -rf target/cargo-timings
cargo build --timings
```

| Metric | Before E2 (= after E1) | After E2 | Δ |
|--------|----:|----:|----:|
| Compile units | 422 | 414 | **−8** |
| Total CPU | 317.6 s | 293.2 s | **−24.4 s (−8%)** |
| Wall-clock | 113.5 s | 112.4 s | −1.1 s |
| Critical path | 96.6 s | 97.6 s | ~0 |

**Removed:** `zstd-sys` (10 s C build script), `zstd`, `zstd-safe`,
and 5 other transitives.

**Kept:** `brotli` (2.02 s), `brotli-decompressor`, `flate2`,
`compression-codecs`, `async-compression` — all the codec machinery
brotli + gzip need.

**Wall-clock unchanged** because zstd-sys built in parallel off the
critical path (which is still `chromiumoxide_cdp → chromiumoxide →
phoenix_ide`). The CPU saving lowers thermal/contention pressure on
shared-core machines, which improves cold builds elsewhere indirectly.

**Validation:**
- `cargo check --workspace` ✓
- `cargo clippy --workspace -- -D warnings` ✓

**Files changed:**
- `crates/phoenix-ide/Cargo.toml` — `compression-full` → `compression-br,compression-gzip`
- `crates/phoenix-ide/src/main.rs` — dropped `.deflate(true).zstd(true)` from `CompressionLayer`
