# Centralize client tool activity projection

Derive a shared pure none/executing/cancelling projection from existing ConversationState. Executing exposes the existing active tool and a readonly queued-call view; cancelling has no queue. Migrate live client consumers without changing backend, wire, persistence, scheduling, labels, timers, historical compact strips, or React memo boundaries.

Acceptance: preserve exact status labels/fallback names/queue counts; nonempty active ID validation in MessageList; exact-tool timers and final-result precedence; latest-message queued-warning suppression; reconnect and token/heartbeat/row render isolation. Validate with focused existing tests and applicable UI check lanes. No merge or deployment.

## Acceptance evidence

Source commit: `75abc99ef52ea701ed71ef7a42044d303625e5e8` (base `b32b50e13240407ecdb491c562061c901f528e04`). The shared projection has no stored state or effects; cancellation structurally excludes the queue. MessageList continues passing a primitive active ID. Historical compact-strip derivation, protocol decoding, and timer ownership are unchanged.

- Focused Vitest: 8 files, 553 tests passed, covering MessageList, MessageComponents, StateBar, atom/reconnect, token/heartbeat isolation, and fixture scenarios, plus uncovered name/queue fallback and malformed active-ID edges.
- `./dev.py check --lanes tsc,ui-lint`: TypeScript, ESLint, and Stylelint passed.
- `./dev.py check --lanes vitest,ast-grep,spec-anchors,task,pkglock`: full Vitest suite, spec anchors, task validation, and lockfile checks passed. This invocation initially skipped ast-grep because the executable was not installed.
- `npm exec --yes --package=@ast-grep/cli -- ./dev.py check --lanes ast-grep`: structural lint, manual SQL transaction validation, and Rust-test-timing validation all passed using a temporary CLI package; no global installation.
- Default `./dev.py check-plan` requires exactly ast-grep, pkglock, spec-anchors, task, tsc, ui-lint, and vitest; every applicable lane passed. Allium/spec-shape are excluded for no spec changes, and E2E is excluded for no Rust/E2E changes.
- `LADLE_PORT=61129 ./dev.py qa tool-results`: all 14 stories at desktop/mobile (28 Chromium captures) passed. Inspected full desktop and compact mobile shell fixtures: active timer remains distinct from finalized results. No production interaction.
- An initial unbudgeted full-suite invocation had one streaming-disclosure timeout in MessageComponents. The owning module passed in the focused run, and the budget-four full-suite rerun passed. The timeout did not reproduce; its cause is not established.

Harness-local logs and screenshots: `.phoenix/tool-activity/`. The isolated rendered-fixture runner exited successfully and its Ladle server on port 61129 stopped; no agent-browser session was created. Independent source review found no actionable issues for base-to-source commit. PR publication is owned by the parent chat; no merge or deployment is part of this task.
