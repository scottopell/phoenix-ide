# Audit E2E quality and eliminate flake-prone synchronization

## Goal

Produce a complete, evidence-backed inventory of Phoenix's end-to-end and workflow-level test synchronization, then fix the highest-value flake risks by replacing elapsed-time bets with observable readiness/completion contracts. A timeout remains appropriate as a final hang bound; it must not be the mechanism that makes a passing assertion true.

This task follows the deploy hotfix separately so emergency scope stays small. It includes the real-binary API harness, browser integration tests, UI workflow tests, and QA capture scripts—not every production timer in the repository.

## Known starting points

The initial triage already found these candidates:

- `tests/e2e/run.py`
  - startup readiness polls `/version` with a bounded ceiling;
  - most scenarios poll authoritative conversation state/transcript evidence;
  - `mid_stream_cancel` has 10-second start and 5-second settle loops;
  - request/read/scenario/cleanup ceilings are duplicated and weakly classified;
  - `_free_port()` followed by process bind is a retry-mitigated TOCTOU race;
  - diagnostics often report only state, not the exact awaited witness.
- `crates/phoenix-tools/src/browser/tests.rs`
  - fixed 100ms sleeps around console-listener registration/capture;
  - screencast tests use repeated 500ms receives and a 100ms post-drop pause;
  - delayed-DOM fixtures and page promises need classification as intentional behavior drivers versus synchronization bets.
- `ui/src/pages/NewConversationPage.workflow.test.tsx`
  - `settleValidation()` sleeps 350ms instead of waiting for validation evidence.
- `ui/src/hooks/useConversationPrStatus.test.tsx`
  - zero-delay timers flush React work and should be assessed against `act`/`waitFor`/microtask alternatives.
- `ui/scripts/capture-ladle-surface.mjs` and fixture renderers
  - polling/deferred interactions need review for durable readiness attributes and teardown completion.

The audit must verify these findings and discover additional surfaces rather than treating this seed list as complete.

## Audit method

1. Define a synchronization taxonomy and acceptance rubric.
   - **Readiness handshake:** explicit event/state proves setup is complete.
   - **Completion witness:** assertion-specific durable evidence proves the operation completed.
   - **Behavior driver:** virtual or controlled time is intentionally the behavior under test.
   - **Polling transport:** repeatedly observes a durable predicate; acceptable when no event seam exists, with adaptive/bounded diagnostics.
   - **Safety ceiling:** timeout only detects a hang and never creates the success condition.
   - **Timing bet:** sleep/deadline assumes work should have happened; must be removed or explicitly justified.

2. Inventory all E2E/workflow surfaces and every timing primitive they use.
   - Cover real-binary API E2E, browser/CDP integration tests, React workflow tests, Ladle/QA capture scripts, subprocess startup/teardown, ports, SSE/WebSocket waits, and retries.
   - Record file/symbol, awaited condition, current primitive, failure symptom, load sensitivity, diagnostic quality, owner contract, and recommended replacement.
   - Exclude ordinary unit tests and production timers unless they directly provide or obstruct an E2E synchronization seam.

3. Rank findings by flake likelihood and impact.
   - P0/P1: fixed sleeps used as readiness, transient predicates that can be missed, shared mutable resources, wrong-turn/wrong-request completion, teardown races, and retry paths that can mask contamination.
   - P2: polling durable state with arbitrary narrow ceilings, duplicated timeout policy, poor witness diagnostics.
   - Accepted: explicit readiness/completion plus a generous outer safety bound, or virtual-time tests of timer behavior.
   - Correlate candidates with recent flake-fix history so recurring bug classes receive structural fixes rather than another threshold bump.

4. Implement a focused first remediation tranche.
   - Replace the browser console-listener sleeps with an explicit subscription-ready/captured-event handshake.
   - Replace `settleValidation()` with Testing Library observation of actual validation completion/UI state.
   - Replace screencast post-drop sleeping with lifecycle completion evidence if the production API can expose it without test-only ambiguity.
   - Refactor real-binary E2E helpers toward typed operation-specific witnesses (turn completed for message ID, tool result persisted, cancellation acknowledged and settled) and richer timeout diagnostics.
   - Keep changes incremental; do not build a generic wait framework unless at least three concrete call sites share the same semantic contract.

5. Add prevention.
   - Add a lightweight structural check for new naked sleeps in designated E2E/workflow paths, with narrow allowlisting for behavior-driver or virtual-time cases.
   - Document the rubric next to the E2E contributor guidance so new scenarios name their readiness and completion witnesses.
   - Ensure allowlist entries explain the tested timer behavior and fail review if used merely to settle unspecified work.

6. Stress-verify.
   - Establish a repeatable contention run that overlaps the E2E surfaces with CPU-heavy checks or uses controlled CPU throttling.
   - Run enough repetitions to report raw pass/fail counts and duration distributions before and after each remediation; do not claim flake elimination from one green run.
   - Exercise failure injection: missing terminal event, delayed listener setup, cancellation during provisioning, dropped/lagged SSE, and teardown under load.
   - Run `./dev.py check` and the production pre-deploy path.

## Deliverables

- A checked-in audit report or task appendix listing every reviewed timing primitive, classification, severity, and disposition.
- Code/tests for the first high-risk remediation tranche.
- A prevention guard and concise contributor guidance.
- Follow-up tasks for significant architectural seams that cannot be safely fixed in this tranche; no discovered high-risk item may disappear as “out of scope” without being recorded.
- Raw stress-run evidence sufficient to distinguish structural improvement from a lucky run.

## Correctness constraints

- Do not remove hard hang ceilings; separate them from success synchronization.
- Do not solve flakes by broadly increasing timeouts, adding retries around assertions, or rerunning failed scenarios on contaminated server state.
- Prefer durable domain witnesses over DOM absence, generic `idle`, or “next event” predicates.
- Tie asynchronous completion to stable identity (conversation, message, request, browser session, frame subscription) wherever concurrent operations can overlap.
- Test-only configuration must be structurally scoped and must not permit production to violate specified keepalive/watchdog behavior.

## Expected outcome

Phoenix has a traceable E2E quality baseline, the highest-risk timer bets are replaced by explicit contracts, new naked sleeps are difficult to introduce accidentally, and failures under load identify the missing witness or product defect instead of presenting as arbitrary elapsed-time flakes.

## Audit findings

Synchronization is classified as one of: readiness handshake, completion witness, behavior driver, polling transport, safety ceiling, or timing bet. A safety ceiling may remain around a durable witness; elapsed time must not create success.

| Surface | Primitive and classification | Risk | Disposition |
|---|---|---:|---|
| Browser console capture | Two 100 ms sleeps around CDP log emission; timing bets | High | Replaced with event-count notification from the capture buffer. Listener subscription already completes before session publication. |
| Browser screencast reattach | 100 ms sleep after last viewer drop; timing bet masking async stop/start overlap | High | Removed the ineffective sleep. Follow-up task 36012 makes broker lifecycle session-owned with an awaitable stop→restart transition; a shared task slot alone cannot close concurrent drop/attach ordering. |
| New conversation workflow | 350 ms real timer used to settle directory validation; timing bet | High | Replaced with exact-path validation and dependent Git metadata request witnesses while preserving the no-status-flash initial UX. |
| PR status hook tests | Four `setTimeout(0)` queue drains; timing bets | High | Replaced with Testing Library waits on visible stale-result/error postconditions. |
| Workflow React state flushing | Passing tests emit asynchronous updates outside `act(...)`; incomplete settlement witness | Medium | Follow-up task 36015 identifies and drains each async producer, then makes unexpected warnings fail the suite. |
| E2E mid-stream cancellation | Fixed-cadence snapshot polls around cancel; polling transport with negative readiness predicate | High | Follow-up task 36013 drives cancellation from a positive, identity-bound streaming/tool event. The current durable idle witness is retained until that seam exists. |
| E2E snapshot scenarios | 100 ms GET polling with state plus transcript predicates; polling transport over durable completion witnesses | Medium | Accepted provisionally. Migrate scenario-by-scenario to exact message-id SSE barriers; do not replace with a generic wait framework. |
| E2E create/stream attach | 404 retry with event-loop yield; polling transport around preallocated conversation/message identity | Medium | Accepted provisionally. Server-side pending-stream attachment would remove the remaining shell-visibility race. |
| E2E startup | `/version` probe loop and 60 s outer bound; readiness handshake plus safety ceiling | Medium | Accepted. The probe exits on process death and does not use elapsed time as success. A bootstrap-ready field is preferable if startup gains deferred initialization. |
| E2E port allocation | Free-port probe followed by process bind and bounded retry | Medium | Accepted mitigation. Passing an owned listener would require a server startup contract change. |
| Browser selector/click waits | 100 ms DOM polling until explicit predicate or timeout | Medium | Accepted polling transport; a page-side MutationObserver is a possible optimization, not current evidence of flakes. |
| Browser typing focus | 50 ms post-focus sleep in production tool path | Medium | Follow-up task 36014 verifies `document.activeElement`/selection before typing. This is product behavior, not only test synchronization. |
| Ladle capture startup | 250 ms HTTP readiness polling with 30 s bound | Medium | Accepted polling transport at a process boundary; retain the fixture's durable settled marker for browser readiness. |
| Screencast first frame | Channel receive with 5 s outer bound | Low | Accepted event-driven completion witness plus safety ceiling. |
| Promise/selector delayed fixtures | Page timers intentionally create asynchronous behavior | Low | Accepted behavior drivers; they do not settle the assertion. |
| Fake-timer polling/gesture tests | Virtual advancement of specified debounce, poll, retry, settle, and long-press behavior | Low | Accepted behavior drivers when using async timer advancement and visible/call-count witnesses. |

### Recurring history

Recent flake fixes repeat four bug classes: fixed-delay readiness, event drains that finish before state settlement, async completion matched without stable identity, and teardown/restart overlap. Structural fixes consistently use readiness markers, exact message/token identity, visible persisted postconditions, or awaited teardown. Timeout increases alone are mitigations and remain audit candidates.

### Prevention

`tsx-no-real-timer-settle` rejects real `setTimeout` sleeps inside TSX tests. Tests must wait for observable state or use fake timers when elapsed time is itself the behavior under test. Existing Rust fixture timers and Python process-boundary polling are classified above rather than globally banned.

### Stress evidence

- Browser console and screencast focused tests: 3 consecutive runs each (6 executions) passed without readiness/post-drop sleeps.
- New-conversation workflow, directory picker, and PR status hook: 3 consecutive batches (96 assertions) passed without real-timer settling; workflow `act(...)` warnings are tracked in task 36015.
- Full `./dev.py check` passed all 18 applicable checks in 87.5 seconds at load average 62.3/46.3/26.2; E2E passed in 41.7 seconds.
