# Bounded history real-server qualification

This harness-only branch does not alter PR898 or its frozen application source `3a7ea7e4484ab984370bf6b784856434d389ac3a`. Application and harness identities are separate in receipts.

The narrow `qualify-history-realserver` job in the existing CI workflow runs only on the trusted same-repository `test/continuous-history-realserver-qualification` PR branch. It checks out the immutable app and the exact harness head independently, uses existing action/dependency versions (Rust1.95.0, Node26, pinned pnpm/Playwright), builds matching UI, installs Chromium, and builds the app via `./dev.py check --lanes e2e`. No dispatch/release/signing, credentials, public preview, deployment or production data access. Existing CI concurrency applies; total job bound30minutes, runtime bound900seconds, browser bound300seconds, artifact retention7days.

After an independently prepared app build, exact commands are:

```sh
APP_ROOT=/absolute/pinned-app uv run tests/e2e/history_browser.py --build-receipt /absolute/history-build.json
APP_ROOT=/absolute/pinned-app APP_BUILD_RECEIPT=/absolute/history-build.json \
  uv run tests/e2e/history_browser.py --output /absolute/new/history-evidence --timeout 900
```

Compiler-free checks:

```sh
uv run tests/e2e/history_browser.test.py
node --test ui/scripts/qualify-history-realserver.test.mjs
node --check ui/scripts/qualify-history-realserver.mjs
```

The Python driver imports only frozen E2E server helpers, never their main/build. It validates full clean app SHA, standalone/UI file hashes, tree/lock/toolchain and independent harness identity. Build provenance is an attestation of the reviewed job, not something binary hashes prove alone. Vite preview serves that hashed built UI and proxies real API/SSE to the isolated backend. Backend, preview and browser are loopback-only/owned; temporary HOME/config/data/SQLite and mock LLM exclude personal provider secrets. Owned children are bounded/terminated; startup/backend/browser logs, API/SSE/geometry/assertion receipts, screenshots, isolation and cleanup receipts survive in retained evidence.

Synthetic setup uses actual create/chat/exhaustion/continue endpoints for ordinary and real Global Coordinator aggregates, over201 persisted rows and at least3members each. Aggregate paging is compared exactly with per-member persisted occurrences minus represented generated openings, not just row counts. The browser witnesses genuine cursor acquisition, original-boundary navigation, pin/query preservation, Back/Forward, historical reading during live append and fourth continuation, newest composer sends, rejected foreign/duplicate pins and offline older-load failure/Retry. No request fulfillment, injected snapshots, fake EventSource or direct database mutation.

Anchor proof uses identical virtual-row identity and two-CSS-pixel agreement over four successive frame-backed samples after DOM adoption, not merely response arrival. Enabled breadcrumb timing is observed without an invented latency threshold or a claimed15second fix. The result reports uncovered capabilities separately: deterministic delayed-page supersession, ThinkAside exact-tool targeting when the frozen DOM lacks a tool target, non-Chromium/physical devices and real providers remain unqualified. Hosted browser execution cannot satisfy the later user iPhonePWA/trustedURL gate.

Independent pre-execution review found four harness issues (clipped nav probe, ambient toolchain identity, incomplete paging oracle, pre-render anchor false positive); each was corrected and re-reviewed with no remaining blocker. Light tests pass13Python/6Node; they are not runtime acceptance. Preserve failed hosted receipts and diagnose before any successor run; no blind retry.
