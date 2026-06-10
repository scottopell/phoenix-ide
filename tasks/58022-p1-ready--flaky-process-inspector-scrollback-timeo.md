# Flaky UI test: ProcessInspectorPanel "caps accumulated entries at the scrollback bound" times out

ZERO TOLERANCE: a second flake red-failed CI on an unrelated PR (#249, run
27275750355, job 80556522550) and forced a manual re-kick. Fix it so it is
deterministic — do not just re-run it.

## Symptom

`ui/src/components/ProcessInspectorPanel.test.tsx:174`
("caps accumulated entries at the scrollback bound, dropping the oldest while
keeping the newest") intermittently fails with:

    Error: Test timed out in 5000ms.

The test seeds one line then drives many polls (CAP = 5000) under fake timers so
the running total of appended lines exceeds the UI scrollback cap, asserting the
oldest are dropped while the newest are kept.

## History

This test already has a flake-fix behind it — `0e692ac` "fix: drain microtasks
per timer tick in ProcessInspectorPanel tests (#243)". That reduced but did not
eliminate the flake: it still times out intermittently in CI. The fake-timer +
polling + microtask-draining loop is still racy under load.

## Fix direction (make it deterministic)

The 5s timeout is being exceeded because the test advances a large number of
poll ticks and awaits microtask draining between each; under a loaded CI runner
that loop occasionally doesn't settle in time. Options, in preference order:

1. Drive the cap deterministically without thousands of real awaited ticks —
   e.g. inject/append the scrollback in fewer, larger synchronous batches, or
   expose the accumulation so the test asserts the cap logic directly rather
   than by replaying CAP polls in real time.
2. If the poll-replay shape must stay, make the advance fully synchronous under
   fake timers (no per-tick awaited microtask flush that can starve), or raise
   the per-test timeout AND prove the loop is bounded (a raised timeout alone is
   not acceptable — it masks, not fixes).

The bar: ProcessInspectorPanel's test suite must pass deterministically with no
timing-dependent timeout, on a loaded runner.
