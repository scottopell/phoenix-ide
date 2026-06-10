# Flaky browser test: test_browser_profile_react_no_profiling_build_path depends on the live unpkg CDN

ZERO TOLERANCE: this flake failed CI on an unrelated PR (#249, run 27272821204,
job 80546399869) and forced a manual re-kick. A test that can red-fail a green
change is a defect, not noise — fix it so it is deterministic, do not just
re-run it.

## Symptom

`phoenix-tools browser::tests::test_browser_profile_react_no_profiling_build_path`
panics intermittently at `crates/phoenix-tools/src/browser/tests.rs:2870`:

    navigate failed: Navigation failed: Request timed out.

after burning the full 30s navigation timeout.

## Root cause

The test serves an HTML page that pulls React/scheduler/react-dom UMD bundles
from `https://unpkg.com/...` at navigate time:

    <script crossorigin src="https://unpkg.com/react@18.3.1/umd/react.production.min.js"></script>
    <script crossorigin src="https://unpkg.com/scheduler@0.23.2/umd/scheduler.production.min.js"></script>
    ... react-dom.production ...

When unpkg is slow or unreachable from the CI runner, navigation never settles
and the test times out. The `require_network!()` gate only asserts that *some*
network exists — it does not (and cannot) guarantee unpkg is fast/up, so it does
not protect against this. Any test reaching a third-party CDN on the hot path is
inherently flaky.

## Fix direction (make it hermetic)

Vendor the three UMD bundles as test fixtures and serve them from the test's own
local server (`server.url()`) instead of unpkg — same script order
(react -> scheduler -> react-dom.production), zero external network on the nav
path. Then the test no longer needs `require_network!()` for this reason.

Audit sibling tests in `crates/phoenix-tools/src/browser/tests.rs` for the same
unpkg pattern and convert them all; check for other `require_network!()` /
external-URL uses across the browser test suite while here. The bar: the browser
test suite must pass deterministically with no off-box network dependency.
