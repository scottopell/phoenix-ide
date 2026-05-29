`tools::browser::tests::test_browser_resize_local` can hang indefinitely, stalling the whole `cargo test` lane until the 600s `./dev.py check` timeout kills it.

## Symptom
Observed twice on 2026-05-29 (during PR #155 merge verification):
- `./dev.py check`: the `cargo test` lane hit its 600s timeout; nextest showed `SLOW [>300s] tools::browser::tests::test_browser_resize_local` climbing with no other failures.
- Bare `cargo nextest run`: same test went `SLOW [>60s] … >300s` and never returned (had to be killed).

The tmux flake (task 62006) is a DIFFERENT issue — `fresh_session_starts_in_supplied_cwd` passed in 0.089s in the same runs. This is browser-specific.

## Why it hangs
The test (crates/phoenix-ide/src/tools/browser/tests.rs:770) awaits a sequence of browser tool ops — `BrowserNavigateTool.run`, `BrowserResizeTool.run`, `BrowserEvalTool.run` — each with NO internal timeout. If any underlying CDP/browser-subprocess op blocks (slow chromium launch, lost CDP socket, contention when run alongside the rest of the suite + e2e + UI lanes), the `.await` never resolves and the test hangs forever.

Browser tests are chromium-gated via `PHOENIX_SKIP_BROWSER_TESTS` (tests.rs:25,39): when no chromium is found they skip. On a dev machine with chromium present they RUN — so this hang shows locally and on any CI runner that has chromium. CI runners without chromium auto-skip and never see it (which is why the GitHub gate can stay green while local `./dev.py check` hangs).

## Fix directions
1. Root cause — bound the browser tool ops: give `BrowserNavigateTool`/`BrowserResizeTool`/`BrowserEvalTool` (or the shared CDP-call layer) a per-op timeout so a stalled browser op returns an error instead of blocking the caller forever. This helps real conversations too, not just the test.
2. Test-level guard: wrap the awaits in `tokio::time::timeout` so the test fails fast with a useful message instead of hanging the lane.
3. Infra blast-radius mitigation (does NOT fix root cause): add a nextest `slow-timeout = { period = "60s", terminate-after = N }` in `.config/nextest.toml` so ANY hanging test is killed + reported as a timeout failure rather than stalling the whole run for 600s. Would also bound the tmux flake (62006). Decide separately — it can mask new hangs.

Prefer (1); use (2) as the immediate test stabilizer.

## Notes
- Pre-existing; predates PR #155, which touches no browser code. Surfaced because PR #155's work involved repeated `./dev.py check` runs on a chromium-equipped machine.
- Repro locally: `cargo nextest run -E 'test(test_browser_resize_local)'` (with chromium available; do NOT set PHOENIX_SKIP_BROWSER_TESTS).
- Validation: run the targeted test in a loop (10+) under concurrent load (e.g. alongside `cargo nextest run`) and confirm it always completes (pass or fast timeout-error), never hangs.
