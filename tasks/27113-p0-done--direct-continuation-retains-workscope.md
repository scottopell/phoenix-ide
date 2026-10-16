# Preserve Direct continuation WorkScope identity — v0.13 release blocker

Ordinary Direct/non-Git context continuation must keep the same persisted WorkScope, environment and resource ownership while creating only a new transcript in the same ProductConversation. ADR-026 and REQ-BED-030 are authoritative; new non-Git creation still uses Direct, so this affects current creation, not just historical data.

Acceptance: atomic continuation reuses scope with concurrent/replayed requests; exact environment and scope counts stay unchanged; current/live-owner scoped Bash resolves after predecessor handoff; shared resource routing/cleanup preserve one ownership identity; ordinary and Coordinator behavior remain distinct. Add focused regressions, update contradictory tests/docs, independent review, and exact-head hosted qualification. Preserve PR921; no historical cwd-based scope coalescing, hand DB repair, production migration, merge or deploy. Heavy local compilation requires actual host admission; use bounded hosted qualification.


## Delivery evidence

- PR: [#926](https://github.com/scottopell/phoenix-ide/pull/926), branch `fix/direct-continuation-retains-scope`, base `35548e2c4cef2b6bb99d0db0cc18cd8086e16ff0`. Existing PR #921 contribution and its branch were preserved; work stayed in independently Phoenix-allocated checkout `c8e60468-104e-41db-87c7-0d7f08ebd931`.
- Qualified source: `3bcab0233f8c5e0c659182f6d5a0a8208092f9f4`; [CI 38952790226](https://github.com/scottopell/phoenix-ide/actions/runs/38952790226) terminal SUCCESS on that exact head. Rust, genuine codegen/stale guard, musl check, Clippy, UI/specs, real-binary E2E, task validation and worker-budget source validation passed. Final record-only head check state is recorded in the PR, not borrowed from this prior head.
- No local heavy compile, production database repair, migration, scope coalescing by cwd, merge, or deployment performed.

## Implemented invariant and consumers

`continue_conversation_inner` retains the selected ordinary parent's exact persisted scope. It no longer allocates a Direct replacement or picks an unordered member of a historical product ownership set. Coordinator singleton handling, one-transaction continuation reservation/edge/child creation, Close admission fences, and idempotent winner reuse remain intact.

The terminal handler now chooses teardown by resource identity, not `worktree_path`/Direct mode: persisted Work PTYs survive transcript exhaustion/handoff, while unattached fallback retains conversation-lifecycle teardown and durable scope retirement remains authoritative. Scope-owned terminal Close wiring shares the existing registry and explicit bounded retirement.

Existing legacy tmux adoption now receives the earliest ordinary transcript attached to the exact persisted scope, resolved from ownership/topology. The hint is separate from actor/root identity and threaded through tool dispatch, terminal attach and Close discovery. No filesystem/cwd inference, socket movement, historical ownership repair or new cross-version policy is introduced. A current canonical socket still wins under the existing registry contract.

## Passed tests and coverage limits

- DB: exact Direct scope/environment/product identity and unchanged scope counts; forced pre-reservation two-party race plus replay; historical multi-scope product selects the parent's attachment without modifying earlier scopes; two-hop exact-scope legacy hint.
- Resource consumers: two-hop live-owner scoped Bash resolution, stable Work resource authority with changed actor, persisted cleanup liveness, actual Direct continuation Close inventory capturing one original scope and Bash/tmux/browser identities with no Worktree deletion resource.
- Real PTY: registered shell survives ContextExhausted before successor creation and HandedOff afterward; successor reclaim keeps identical Arc/fd/PID/tracker/history. Explicit retirement fences attaches/spawns, closes/removes PTY, and reaps child even without a relay attachment. Tests use real PTYs but not a full axum/WebSocket or aggregate-Close E2E session; tracker seed and lifecycle event broadcasts are test-controlled.
- Legacy tmux: fresh registry over a fake backend adopts predecessor socket without spawn and preserves server/token/process/window view; explicit absence does not adopt actor socket. These are fake-backend tests, not production tmux process proof.
- Existing real-binary non-Git creation/context-exhaustion/continuation E2E now asserts actual Direct mode, exact original scope, cwd and full resolver environment equality. It is not an old-binary downgrade test or production deployment proof.
- Bash/browser/tmux/terminal requirements and Allium agree with ADR-026/REQ-BED-030. Scope handoff retention is explicitly distinct from aggregate retirement: same-aggregate continuations/subagents do not veto Close; distinct still-Open aggregate ownership and identity conflicts retain their prior authority boundary.

## Review dispositions

Independent frozen review `35548e2c4..b849dff40` found no confirmed source defect and identified race-test coverage limits; the Direct race was strengthened with the existing deterministic reservation barrier. Exact test delta through `3fcfecf88` found no actionable defect. Subsequent independent spec review found a row-vs-aggregate cleanup ambiguity; `86baae272` clarified it, with independent repair review finding no new issue.

Hosted review findings were inspected individually, not treated as approval:
- Browser normative Direct teardown contradiction: validated and corrected across browser/Bash/tmux/resource contracts.
- Tmux model/REQ-001 transcript-vs-scope identity and helper semantics: validated; all entity fields, creation paths, lifecycle selectors and invariants now use persisted scope. Independent review caught an incomplete first model edit; fixed in `eea105217`/`c008f5739` and re-reviewed. Black-box helper semantics explicitly documented.
- Old public browser/tool identity comments: corrected without adding distributed design comments.
- Actual Direct PTY listener teardown: validated runtime P1 and fixed; independent anchored review found no correctness defect, verified registry/Close wiring, and retained real-WebSocket coverage limits.
- Legacy tmux predecessor adoption after restart: validated P1 and fixed through the exact-scope persisted hint. Independent anchored review found two introduced compile errors (missing Unattached arm and owned/borrowed argument mismatch), both corrected in `59e6152dd`; narrow follow-up found no remaining compile-wiring issue. Subsequent test fixture missing `phoenix_terminal::ActiveTerminals` qualification was fixed in `3bcab0233`, then full CI passed.

Reviews were bounded static evidence, not exhaustive proof. Anchored passes were labeled when external topics were supplied. Exact final record-only delta and terminal hosted checks are reported in the PR.

## Retained failures and follow-through

- First hosted failure [38937101483](https://github.com/scottopell/phoenix-ide/actions/runs/38937101483): exhaustive match lint in authored test; corrected. E2E enum projection was aligned to actual `conv_mode.mode = Direct` rather than guessed wire keys.
- Source/test head `86baae272` passed full [38940260178](https://github.com/scottopell/phoenix-ide/actions/runs/38940260178); later review uncovered additional consumers, so this success was not reused as final qualification.
- Actual PTY repair source `35af6f889` passed full [38948755287](https://github.com/scottopell/phoenix-ide/actions/runs/38948755287); later legacy-adoption changes required fresh qualification.
- Legacy repair's first independent compile blockers were corrected before final source validation. [38952016341](https://github.com/scottopell/phoenix-ide/actions/runs/38952016341) failed on the authored fixture type import; corrected, followed to exact successor terminal success [38952790226](https://github.com/scottopell/phoenix-ide/actions/runs/38952790226).
- Intermediate superseded/cancelled runs remain in PR branch history; they are not success evidence. No wait was left at a vague pending state. No local heavy-build fallback was used.

## Release admission boundary

Implementation and source qualification are complete. Global must independently admit merge/deploy and clear the v0.13 blocker against the integrated exact source. No current production behavior, historical scope repair, data migration, remote federation completion or deployed verification is claimed by this task status.


### Final review closeout additions

The later tmux user-facing isolation description and Bedrock rationale were still contradictory despite the earlier lifecycle fixes. `fc25b67fea5d1f57f9d3344af11e0844af36e94a` corrects the actual tool description and REQ-TMUX-009/SocketIsolation to scope—not transcript—isolation, and distinguishes the selected leaf attachment from the product's historical scope set. Independent exact-delta review found no new contradiction; the tool-description string changed but execution logic did not. Terminal-opening guidance was already corrected to persisted environment lookup; the later quoted removed Worktree mapping was checked against actual source rather than assumed current from retargeted GitHub comment metadata.

Source-qualified `3bcab0233` remains retained success evidence. The final published record head's CI and all nine hosted thread dispositions are recorded in PR #926 before Global handoff; task completion is implementation delivery, not authority to merge/deploy or independently clear release admission. No open finding is dismissed solely because an earlier CI run passed. Documentation-only/description successors and their actual check results remain visible in PR history, including the superseded/cancelled record-head run 38955370096.
