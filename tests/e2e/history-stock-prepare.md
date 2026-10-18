# Exact f16 stock joint preparation packet

This packet prepares a hosted **cfg(test) library** witness, not a shipping binary
or production acceptance. No app source, telemetry thresholds, sample rate,
provider configuration, or frozen db6 SQL qualification changes are included.
The source remains `f16afb227d9336a863c0b0345960ea9a6d62b7c7`, tree
`369305dcd94862d4ee12e7af9fd463d5da14d808`.

## Activation — parent review first

The `qualify-history-realserver` job activates only for a same-repository PR
whose head is one of its existing allowed branches or
`prepare/exact-f16-stock-admission`. On the new branch only, its stock step
invokes the existing `tests/e2e/history_opening_browser.py --joint-telemetry`
CLI **once**, without `--controlled-functional`. Existing allowed branches keep
their controlled runtime command and exact prior raw artifact upload allowlist. Chromium installation is required by both;
the additional native Chrome teardown/layout repeat is skipped on the prepare
branch. Nothing in this packet executes the hosted workflow locally.

After review, publishing this branch and opening/updating its same-repository PR
activates the lane. The workflow's top-level triggers are unchanged: branch
pushes alone do not activate CI (`push` is restricted to `main`); PR events also
run the ordinary check-plan/check jobs as before. This packet does not suppress
normal checks or introduce a broad push trigger.

## Receiving artifact and driver ownership

The order is stock Vite build → genuine existing typed codegen → existing API/DB
qualification → default-feature instrumented libtest selection/tests → retain
`opening-stock-libtest` → separate controlled Vite build → real two-build owner
binding → full existing UI gates → required Chromium preparation → stock CLI.
Thus stock `ui/dist` is present before any receiving libtest compilation and
before the controlled build. The retained executable is selected from Cargo
compiler-artifact metadata, not from a guessed target filename.

`opening-stock-receiving-build.json` records the stock executable/UI hashes and
build configuration. `opening-stock-instrumented-receipt.json` includes the real
stock **and** controlled build-owner binding required by `identity()`. Its
receiving mode remains stock; it is not relabelled as controlled to bypass that
check. The CLI receives absolute app/executable/receipt/output paths. It verifies
all four immutable telemetry module pins, imports the sink/assertion helpers
from the pinned app, and uses the existing authenticated ordinary + Global
fixture, lifecycle `sidebar_home`, owned receiver, and `runTelemetryChecks` route.

The receiving build receipt keeps `stock_ui_files` as the complete raw manifest
inventory and records `stock_served_ui_files` separately for comparison with the
existing instrumented receipt's `ui_files`. Only the root `.gitkeep` placeholder
is excluded from the served map; if present its SHA-256 must be the empty-file
hash. Nested placeholders and other dotfiles fail closed. The full raw map must
still match the admitted stock build-owner inventory, and the served map must
match the existing receipt exactly. Manifest/source-graph hashes and the original
instrumented receipt producer are unchanged.

This is fixture admission **plus bounded natural sampling** at 0.25, not a new
fixture-only mode. The telemetry operation's immutable watchdog is 285 seconds,
with two bounded 5-second cleanup/drain waits (295 seconds of explicit bounds,
below five minutes; not a measured runtime guarantee). The history Node driver
has its existing 500-second per-case wait and 480-second combined history/joint
watchdog. The CLI is additionally bounded to **500 seconds total across both
cases**, within a 10-minute hosted step. The second case may exhaust that shared
budget; timeout, insufficient natural samples, or failed cleanup must remain
failures/inconclusive, not reasons to retry with relaxed thresholds or claim
positive overhead clearance.

## Upload boundary

On the prepare branch only, keep stock `ui/dist`, manifests, receiving
executables, build receipts, and codegen/API/full-UI gate receipts/logs. Its raw
`opening-stock-evidence/` directory is **not uploaded**. The existing controlled
branches retain their prior upload allowlist, including `opening-browser-evidence/`;
they do not invoke the new projection or emit a controlled publication.

`history_runtime_publication.py` reads only enumerated stock JSON receipts into
`opening-stock-publication.json`. It retains every bounded per-trial value of
approved CDP counters before/after navigation, the driver's selected post-readiness
metrics (including heap), navigation-to-visible-readiness timing, trial order,
route, warmup/sampling/reset/failure/cleanup facts, observer counters, producer
admission/mark decisions, per-report numeric marks/totals, terminal/lifecycle
trial inventories, and journey expectations. Numeric values must be finite and
within ±1e12; booleans cannot masquerade as numbers. The initial trial bound is
49 (48 natural trials plus the separately observed history inventory); journey,
producer-event, and CDP-vector bounds are 64, and each report's correlations are
bounded to 16. Overflow counts are explicit, never evidence of completeness.
Only fixed enum strings, hexadecimal identity hashes, UUID correlations, and the
producer's fixed-form `history-{ordinary|global}-<16 hex>` run ID cross this boundary.
No generic string/path map is copied. Admitted app/frontend/server identity,
build evidence class, UI manifest hashes, real collector acceptances, and exported
trace correlations remain available when emitted.

The pinned driver does **not** emit CDP deltas, long-task distributions, post-GC
retained heap, or total wall time; these omissions are explicit publication gates,
not fabricated measurements. Its wall timing includes automation latency and is
not paint timing. Its browser receipt does not emit an evidence-class field;
the projection says so and retains the result/build/delivery evidence classes.
Missing identity or collector/export/run correlations are explicit gates. This
projection does not revalidate the collector join, establish neutrality, apply
new thresholds, or grant overhead/shipping clearance.

Missing, malformed, oversized, or symlinked receipts are explicit non-success
observations. Free-form errors, startup/fixture files, credential identity/files,
authkey, cookies/storage state, databases, screenshots, runtime logs, and raw
OTLP batches never cross the **stock** runtime publication boundary. Synthetic
ephemeral credentials/fixtures are permitted inside the hosted owned run only.

Local validation for this packet is limited to cached static/filesystem/mocked
regressions and syntax checks. No local build, dependency installation, network,
server, browser, SQL, push, or hosted execution is authorized by this packet.
Full `dev.py check`, shipping-feature/runtime capability, physical/native browser
behavior, natural-sample sufficiency, and overhead clearance are not established.

Final bounded integration validation: stock preparation static/filesystem
regressions **17/17**, publication allowlist/distribution regressions **4/4**,
and controlled artifact regressions **8/8**; CI YAML parse and all inline workflow
shell blocks passed `bash -n`. No broader tests were rerun for these integration
fixes. GitHub roadmap context was not re-fetched because this packet's validation
prohibits network access. Parent review is required before any push.

The empty-placeholder fixture reproduces the original raw-versus-served binding
failure. A separate read-only proof against the retained `27e135` stock artifact
passed the new workflow mapping assertions: 454 raw files, 453 served files, only
empty root `.gitkeep` excluded, and unchanged manifest SHA-256
`99700f2b31f4cf322cac450334722f91596ae275382ba531eba58b5b982cdb75`.
No retained artifact was rewritten; this mapping proof is not runtime admission.
