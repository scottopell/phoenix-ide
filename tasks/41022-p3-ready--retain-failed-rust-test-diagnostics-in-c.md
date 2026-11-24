# Preserve failed Rust test diagnostics in broad check output

In the 2026-11-20 tool-discovery check, nextest ran 6,342 tests with fail-fast disabled. Three failures occurred before the final 200 output lines. `./dev.py check` eventually printed only the passing tail and failing test names, losing each failure's assertion/output. It also withheld the failure details until subsequent check lanes completed. Isolated reruns were needed to recover one deterministic assertion; two browser failures did not reproduce, so their original cause is unknown.

Make bounded failed-test details available even when thousands of later tests succeed. Prefer nextest's supported final-failure output or structured summary behavior over keeping every successful test line. Cover the command/output contract in dev.py tests. Preserve existing privacy constraints; do not start printing credentials or production provider payloads.

Observed while validating task 27100. The failed check otherwise completed all 19 stages, with one test-stage failure, and retained only the final 200 Rust-test lines. No check-runner implementation changed in that work.
