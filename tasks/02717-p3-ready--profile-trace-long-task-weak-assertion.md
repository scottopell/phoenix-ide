Test `tools::browser::tests::test_browser_profile_trace_stop_long_task_real`
(crates/phoenix-ide/src/tools/browser/tests.rs:2633) has a silent-pass weak
assertion. Surfaced 2026-05-28 while triaging 3 flake suspects alongside
45001 (browser resize hang) and 62006 (tmux cwd flake).

## CONFIRMED 2026-05-28: it is a 100% false-pass, not occasional

Implemented the tightened assertion (parse count, assert >=1, plus read back
the /tmp/phoenix-trace-*.json and require a dur>50000us event) and ran it on
an idle macOS host with real Chrome. It FAILS deterministically:

    trace must capture the >50ms busy-loop as a long task, got 0:
    Trace saved to /tmp/phoenix-trace-...json (831 events).
    Long tasks (>50ms): 0, total -0.0ms.

Dumped the saved trace: 831 events, 550 with a `dur`, and the LARGEST dur is
323us — three orders of magnitude under the 50_000us threshold. Top events
are all GC (MinorGC, V8.GC_SCAVENGER*). The 120ms `while(Date.now()-t<120){}`
busy-loop produces NO single long-duration event.

Root cause of the false-pass: the busy-loop runs inside a CDP
`Runtime.evaluate` (BrowserEvalTool), and that synchronous execution is not
attributed as a traced main-thread RunTask in the enabled categories
(devtools.timeline / v8 / blink.user_timing). So `long_count` is always 0,
and the old "is a digit" assertion always passed on "0". The test has never
verified long-task capture end-to-end.

## Fix direction (REVISED — workload first, mandatory)

The assertion tightening is correct but CANNOT be merged until the workload
produces a genuinely-traced long task. Likely fix: schedule the blocking
work in a timer callback so Chrome traces it as a discrete RunTask/TimerFire
event, then wait for it to fire before trace_stop, e.g.

    eval: `setTimeout(() => { var t=Date.now(); while(Date.now()-t<200){} }, 0)`
    then wait ~300ms (so the timer fires while tracing is active)
    then trace_stop

VALIDATE empirically: dump the trace and confirm >=1 event with dur>50000us
BEFORE re-adding the >=1 assertion. If a timer callback also isn't traced as
a long task, investigate which category/event Chrome uses for evaluate-driven
main-thread work, or drive the workload from page script instead of CDP eval.

The tightened-assertion patch was written and then reverted out of the
45001/62006 PR specifically because it goes red until the workload is fixed.

## Not a fail-randomly flake — a false-PASS gap

Unlike 45001/62006, this test will not spuriously FAIL. The drain path, the
30s TRACE_COMPLETE_TIMEOUT, and the lazy-`notified().enable()` lost-wakeup
race fix are all correctly implemented (profile.rs:1724-1843). A trace
timeout only appends "(note: tracingComplete timed out; trace may be
partial)" — it does not fail the asserts.

The real defect: the long-task assertion only checks the count is a DIGIT.

    let after = after.trim_start();
    assert!(after.chars().next().is_some_and(|c| c.is_ascii_digit()), ...);

`"0".chars().next()` is a digit, so a count of 0 PASSES. The test runs a
120ms main-thread busy-loop (`while(Date.now()-t<120){}`) intending to
register exactly one >50ms long task. If Chrome does NOT attribute that
busy-loop as a single >=50ms task event (build-dependent, category-sampling
dependent, or split across scheduler yields), `long_count == 0` and the
test passes having verified nothing end-to-end. The comment at line
2667-2668 calls the trace_stop completion "load-bearing" — but the
long-task count, the part that exercises the extraction logic, is not.

## Root cause

Assertion accepts the degenerate value the test exists to rule out. Long-task
count comes from `long_tasks.len()` where `long_tasks` filters trace events
with `dur > LONG_TASK_US` (50_000 us) — profile.rs ~1771-1789.

## Fix direction (correct-by-construction)

Make the invariant the test claims to verify impossible to satisfy with the
degenerate value:

1. Parse the count as an integer and assert `>= 1` (minimum change). A
   busy-loop that fails to register then FAILS loudly instead of passing.
2. Stronger: read back the reported `/tmp/phoenix-trace-*.json` and assert
   at least one `traceEvents` entry has `dur > 50000`. Verifies the
   extraction parsed real Chrome output, not just that a number was printed.
3. Make the workload deterministic so >=1 is guaranteed on every Chrome
   build (investigate whether the default trace categories reliably attribute
   a synchronous busy-loop; if not, drive a workload Chrome always reports as
   a long task).

Prefer (1)+(2) together: assert >=1 AND verify via the file. (3) only if
(1) proves the busy-loop is genuinely unreliable across builds.

## Validation

Run the test 20+ times on a chromium-equipped host under concurrent load
(alongside `cargo nextest run`). Confirm the long-task count is consistently
>= 1; if it is ever 0, fix the workload (direction 3) before tightening the
assertion, else the tightened test becomes a real flake.

## Relationship to siblings

- 45001: browser session-acquisition has no timeout -> true hang. Shares the
  browser tool stack but a distinct root cause (unbounded launch await).
- 62006: tmux spawn_session returns before pane is usable -> true flake.
