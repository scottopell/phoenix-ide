# Investigate browser CLI lifecycle failures under the broad suite

The 2026-11-20 tool-discovery check run reported two failures among 6,342 Rust tests:

- `phoenix-tools::browser_cli dead_chrome_cleanup_failure_remains_visible_until_explicit_close`
- `phoenix-tools::browser_cli lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime`

Both passed an immediate isolated nextest rerun (2/2), then the complete `browser_cli` integration binary passed (37/37, zero skipped) with native Google Chrome explicitly selected and four workers. No browser lifecycle implementation or tests changed in the tool-discovery work. The original assertion details were lost from the check runner's final 200-line tail; do not infer a particular race or claim a proven cause.

Reproduce with the normal broad `./dev.py check` worker budget, preserve exact failing output and browser/process ownership evidence, and fix a demonstrated causal defect. Preserve the existing lifecycle assertions and use observable completion rather than arbitrary sleeps. The focused baseline command is `PHOENIX_SKIP_BROWSER_TESTS=0 PHOENIX_CHROME_EXECUTABLE='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' cargo nextest run -p phoenix-tools --test browser_cli --test-threads 4`.

Observed during source range `15ec903e20eb4e46ad419b07d56c959867fc9a0a..4927b9f78`; qualification evidence belongs with completed task 27100. This is a non-reproducing broad-run failure, not an established tool-discovery regression.
