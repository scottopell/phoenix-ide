`tools::browser::tests::test_eval_complex_page_inner_text` hung once (>120s,
killed at 180s) during the 45001/62006/02717 flake-fix validation hunt on
2026-05-28. Same unbounded-browser-op family as 45001 (resize hang).

## Context / likely environmental

Observed at iteration 20/20 of a flake-hunt running the full `tools::` suite
at 8 threads (8 concurrent chromium) WHILE a foreign cargo build
(task-67006 worktree) was compiling. Machine load average was 34.9 / 109 /
124 on a 10-core host = ~12x oversubscription. Under that thrash even
bounded ops crawl and chromium launches queue. The other 19 iterations
passed; resize (the 45001 target) never hung across all 20.

## Why not certainly a code bug

The eval op IS already bounded: `BrowserEvalTool::run` wraps
`guard.page.evaluate` in a 15s `tokio::time::timeout` (tools.rs:211), and the
45001 fix now bounds session creation at 30s (SESSION_INIT_TIMEOUT). A single
>120s stall is not explained by one bounded op — suspect cumulative slow ops
under load, OR an unbounded await still on the eval/navigate path.

## Investigate if it recurs

1. Audit remaining UNWRAPPED `guard.page.evaluate(...).await` calls in
   tools.rs (lines ~719, ~842, ~975, ~1226) — these poll/wait helpers have no
   timeout. Confirm none are reachable from BrowserEvalTool::run /
   BrowserNavigateTool::run under this test.
2. Reproduce on an IDLE machine (load < ncpu) with `cargo nextest run -E
   'test(test_eval_complex_page_inner_text)'` in a loop. If it never hangs
   idle, this is environmental and can be closed.
3. If it hangs idle: bound the remaining path (per-op or per-tool-call
   ceiling), same correct-by-construction approach as 45001.

## Note

The decision was made (2026-05-28) NOT to add a nextest slow-timeout
terminate-after guard (45001 fix direction 3), to avoid masking new hangs
behind a generic timeout-failure. So a future hang of this class will again
stall the lane until the 600s ./dev.py check timeout — this task is the
tracked follow-up if that recurs.
