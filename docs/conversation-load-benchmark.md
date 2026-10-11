# Conversation-load development benchmark

This is a local read-path development loop, not a production clone or an optimization result. It reuses `browser_profile.run_scenario`; no alternative benchmark runner is introduced.

## Run on this host

```sh
# Each capture needs a new, gitignored directory. Budget several GB and minutes.
./dev.py conversation-load capture \
  --source /Users/scottopell/.phoenix-ide/prod.db \
  --fixture .phoenix/conversation-load-fixture-new

# Builds the normal optimized Vite UI and a debug read-only Rust server.
# Use --release for a separately identified release-backend comparison.
./dev.py conversation-load serve \
  --fixture .phoenix/conversation-load-fixture-new --port 8043

# After builds, --no-build restarts only this fixture server.
./dev.py conversation-load scenario \
  --fixture .phoenix/conversation-load-fixture-new \
  --url http://127.0.0.1:8043/ > .phoenix/conversation-load-scenario.json
```

Read the generated JSON and pass its arguments to `browser_profile`. Defaults deliberately request one exploratory pilot, CPU rate 1, no warmup repetitions and no forced GC. Agree repeated-run counts and performance budgets before creating the benchmark sample set. Do not report significance from exploratory pilots. `--release --no-build` requires an already built release binary.

After the tool returns, read `window.__loadBenchmarkResult` with `browser_eval`. Retain that timing-only result alongside the tool's **raw_samples** and methodology warnings in the ignored private fixture directory. Record:

- fixture database SHA-256 from `manifest.json`, auxiliary-file hash, capture counts and omissions;
- Git HEAD **and dirty diff identity**, backend debug/release profile, UI asset identity, browser/viewport/host;
- first-visible and frame-proxy durations, harness window duration, individual request intervals/payload sizes, long tasks and status;
- correct route, exact expected tail message/sequence, enabled composer and bottom gap;
- successful real PageUp/PageDown interaction without submitting input, followed by restoration of intended anchor;
- setup readiness, warm/cold browser/server distinctions and observer overhead.

The first wait is outside the measurement window. The generated scenario times a real list click and requires the fixture's current tail, nonempty visible content, enabled input and a bottom anchor within rounding tolerance. Two animation-frame callbacks are a **paint opportunity proxy**, not an actual paint measurement or proof of timed scroll responsiveness. Check scrolling separately. A timed-out readiness check is a failed pilot, not a sample. On timeout, navigate/reset before retrying to discard page-local observers.

## Safety boundary

The dedicated `load_benchmark` binary never invokes normal server startup or `AppState::new`. It constructs no runtime, providers, tool registries, recovery, coordinator/wake/creation/direct-turn workers, discovery, worktree reconciliation or periodic background services. dev.py supplies a minimal environment and an isolated HOME/TMPDIR. It binds only loopback.

The database must be a standalone, read-only, regular file without hardlinks or WAL/SHM/journal sidecars. `Database::open_read_only` uses immutable SQLite; it does not migrate. Non-GET requests and unlisted API routes are denied before handler entry, including telemetry, wake, send, terminal and normal runtime SSE. Auxiliary fixtures cannot override the real product list/detail routes. Static fixture SSE emits a supplied init then remains pending without runtime materialization. The model shown by the fixture is a UI-only carrier; it cannot execute. An enabled composer does **not** grant submission authority.

Capture opens the source `mode=ro`, `query_only`, in one consistent read transaction. It creates a fresh schema and inserts only allowlisted read-model rows, not a raw backup. Credentials, jobs, subscriptions, runtime obligations and migration descriptions remain absent. Known content trees are reconstructed; prose, arbitrary tool arguments/results, titles, paths and model identities are synthetic. Unknown content fields/column classifications fail closed. Output directories are private, ignored and local. FK validation, count matching and a manifest identify the resulting fixture. No live DB migration/downgrade or production restart is involved.

**Schema trust precondition:** the source must be a Phoenix-managed database with trusted, nonsensitive schema DDL. Table/index/view definitions are reproduced from sqlite_master (excluding triggers and virtual/shadow tables); this is not a sanitizer for arbitrary user-authored SQLite schemas that embed private literals in DDL. Do not use it on an untrusted/ad-hoc database or publish the resulting schema/data. Fixture data is still private despite synthesized prose: UUIDs, timestamps, relationships, content lengths and tool mix remain.

Stop the fixture's own tmux process before modifying/replacing its DB. Never replace an immutable file under a running server. Keep the capture immutable across before/after measurements. Delete disposable old fixture directories only after their servers stop and any needed manifests/results have been retained. No fixture is committed.

## Real path versus fixture seams

Real production code is reused for:

- `Database::read_ordinary_product_conversation_snapshot` and product list hydration;
- snapshot projection, member aliases, continuation topology, pagination/cursors and serialization;
- the ordinary built UI's route/list-click effects, duplicate requests, transcript rendering and virtualization.

`Server-Timing: db;dur=…, projection;dur=…` on the read-only server identifies primary DB-read and projection time. Projection can include additional DB lookups; serialization/transport are not included. Production handlers use the same extracted helpers but do not gain these headers. No query optimization is included.

Fixture seams: coordinator/member routes, legacy conversation lists, provider/model metadata and static current-member SSE. Omitted attachments, opaque display/usage metadata, non-Markdown synthetic prose, normalized idle states, absent runtime load and a repacked DB change costs. Legacy `/api/conversations` is empty in this first fixture version, unlike production's large response; this is a declared fidelity gap. Static init is an independent sanitized tail read for composer initialization, not production SSE timing. These differences prohibit calling a faster fixture an improvement or claiming absolute production parity.

## Evidence and next calibration

Production localhost exploratory pilots were approximately 3.67 s and 3.49 s to transcript readiness proxies. A valid early development pilot reached the visible tail/enabled composer in 1.479 s and frame proxy in 1.504 s (harness 1.571 s). It issued two small detail responses followed by a larger snapshot; primary DB work dominated, with no >50 ms browser task. The final generated scenario on a fresh sanitized capture reached first visibility in 0.983 s and the frame/anchor proxy in 1.011 s (harness 1.074 s). It used two `message_limit=1` requests followed by `message_limit=100`. These pilots occurred under normal cache state and concurrent repository validation; they are not controlled repeated samples. This is an analogous slow read-path starting point, **not yet a calibrated production-equivalent benchmark**.

Bounded local production trace correlation independently located approximately 2.365 s in `message_page` within an approximately 2.44 s detail read; aggregate was approximately 60 ms. Investigate fixture-versus-production message-page cost, list interference, pool/cache configuration, payload/index/page layout and build identity before selecting optimization scope. Do not add sleeps or pad timing to manufacture agreement. No performance threshold is defined here.

Validation: focused Python safety/consistency/scenario tests, Rust read-only-router/isolation tests, existing product-detail/list tests, real browser readiness and scrolling, then `./dev.py check`. Keep broad-check failures classified; do not silently fix unrelated owners' work.
