# Fix message-list light fixture readiness timeout

`./dev.py qa message-list` reproducibly captures the first six scenarios, including `scroll-policy-long` and prefix continuity with 0px drift, then times out waiting 10 seconds for `[data-message-list-fixture-ready="wide-markdown-table-light"]`.

Observed twice while verifying transcript jitter work. The dark wide-table scenario immediately before it passes its surface and overflow geometry checks. Diagnose whether the light story fails to mount, shares stale fixture state, or needs a readiness contract fix. Keep the marker deterministic rather than increasing arbitrary sleeps.

Acceptance: all seven message-list captures complete repeatedly through `./dev.py qa message-list`.
