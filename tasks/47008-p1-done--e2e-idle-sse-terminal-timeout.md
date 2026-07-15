# Diagnose e2e idle/SSE terminal barrier timeout

The `text_streaming` e2e repeatedly times out after 45 seconds while reporting `last state='idle'`. Harness unit tests for idle and agent-done terminal detection pass, and all Rust/spec gates pass. Reproduce the full server path and determine whether the SSE stream omits/reorders terminal evidence or the harness fails to associate it with the exact user turn. Do not increase the timeout as a substitute for identifying the missing completion signal.
