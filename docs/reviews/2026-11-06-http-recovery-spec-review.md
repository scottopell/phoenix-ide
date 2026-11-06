# HTTP MCP recovery specification review — 2026-11-06

The two review passes below describe the initial draft. The later owner-approved
simplification supersedes the retained strict-cleanup disposition; see the final
section. These were author-performed review passes, not independent agent reviews.

Scope: requirements, Allium, proposed ADR-230 and acceptance scenarios only.
These are two author-performed adversarial passes, not independent-agent or
implementation reviews. Initial draft: commit `6a6372246`. The corrections below
are part of the reviewed successor. No runtime correctness or deployment claim
follows from this review.

## Self-consistency pass

| Finding | Adversarial counterexample | Resolution |
|---|---|---|
| General 404 recovery conflicts with cleanup | DELETE says the old session is absent, but the generic rule creates another session. | REQ-MCP-005 and HttpSessionExpired distinguish non-DELETE expiry from successful already-absent cleanup. |
| OAuth handoff hides live diagnostics | Cleanup 401 hands off to silent refresh, but the retry episode becomes blocked and vanishes from active status. | Active means lifecycle ownership; it can await owned OAuth. Phase and structured last-failure evidence remain inspectable. Browser waiting schedules no connection attempts. |
| Static authentication enters the OAuth handoff | A configured static token gets 401 and starts browser authorization. | Disjoint HttpRecoveryStopsOnStaticAuthorization and OAuth-handoff rules. |
| Fresh-grant rejection permits endless refresh | Refresh succeeds, DELETE rejects the fresh bearer, and the authorized-state rule exchanges another rotating grant indefinitely. | Disjoint fresh-grant rejection and refresh-needed rules; the same fresh rejection requests owned authorization. |
| A failed replacement can leak its new session | Initialize creates a session, tools/list fails, and the next retry initializes again. | Resume the first unfinished step; clean up any partially initialized replacement before another initialize. |
| Retry timing is underspecified | Two implementations retry before a long Retry-After, apply negative jitter, or disagree about malformed values. | Server deadline is a lower bound; exponential 5–60-second client backoff has nonnegative jitter up to twenty percent within the cap; invalid values use backoff. |
| Queued configuration invalidates the cleanup owner too early | A new resource replaces configuration while old-resource cleanup still needs its grant. | Ownership predicate explicitly retains old-resource cleanup through transfer/adoption. Existing credential and configuration fences still govern it. |
| A historical good scan masks broken discovery | Status says healthy despite a failed MCP connection and no tools. | REQ-SG-017 separates present connection health from completed-scan evidence. Recovery never advances the latter. |
| Recovery completion relies on an unrelated signal | Connection becomes ready but an extra completion notification is never emitted, leaving a live recovery episode. | Completion is driven directly by the server's transition to ready. |

Result: no unresolved self-consistency finding in the revised draft. The event
model relies on the declared current-owner and typed-failure boundaries; those
must be exercised by the acceptance matrix during implementation. Structural
Allium validation does not prove asynchronous runtime behavior.

## Highest-Leverage Findings

### 1. Successful cleanup before replacement remains the largest retained cliff

- **Requirement:** Do not publish a replacement until retained session cleanup
  finishes.
- **Why it looks inexpensive:** DELETE appears to be a final housekeeping call.
- **Hidden cost:** Cleanup can need refreshed credentials, required scopes,
  retained grants and recovery ownership even after the old connection is unusable.
- **Architectural consequences:** Authentication and transport cleanup must share
  an owner. The retry cannot simply drop the old transport and initialize again.
- **Blast radius:** Shared MCP lifecycle, reconfiguration and removal already
  depend on this ordering. A remote service that never accepts deletion can still
  block recovery indefinitely; pacing does not guarantee availability.
- **Lower-cost alternative:** Best-effort remote DELETE after proving local
  quiescence, allowing old remote sessions to expire. That deliberately relaxes
  the existing session-ownership contract and needs a separate decision. It would
  reduce the need to retain old-resource grants through queued configuration just
  to finish remote deletion. Local quiescence, credential audience binding,
  completion fencing and uncertain-operation no-replay would still be required.
- **Disposition:** Retain the existing contract. This draft adds a path forward
  after transient failure, not an exception that abandons unresolved sessions.

### 2. Durable retries would turn connection repair into a workflow subsystem

- **Requirement at risk:** Recover every retry timer and old remote session after
  process restart.
- **Why it looks inexpensive:** Persisting a next-retry timestamp looks small.
- **Hidden cost:** Also requires session/grant lineage, crash boundaries, stale
  worker fencing, schema changes and cross-version recovery semantics.
- **Architectural consequences:** A durable connection-job subsystem rather than
  a timer owned by the existing running supervisor.
- **Blast radius:** Database migrations, startup, upgrades and removal; broad
  failure-injection and compatibility tests.
- **Lower-cost alternative:** Process-local episodes and timers; reconstruct
  configured connections on startup using existing stored credentials.
- **Disposition:** Excluded. Existing durable credential-removal intent is neither
  removed nor generalized to remote sessions or new retry jobs.

### 3. Gateway reconnect and alert loops would create competing recovery owners

- **Requirement at risk:** Have Slack repair its MCP server and repeatedly notify
  the owner until acknowledged.
- **Why it looks inexpensive:** Reload when scans fail; send an alert on errors.
- **Hidden cost:** Competing timers and reconnects, shared-server disruption,
  alert deduplication, failed notification transport and new delivery state.
- **Architectural consequences:** Gateway-owned lifecycle and notification retry
  machinery alongside shared MCP ownership.
- **Blast radius:** Other MCP callers, Slack budgets, uncertain writes and the
  deliberately simplified one-attempt notification/eyes contracts.
- **Lower-cost alternative:** One shared MCP recovery owner; current diagnostics
  in native settings plus existing completed-scan evidence.
- **Disposition:** Excluded. No gateway reconnect loop, alert outbox, health-history
  store or revived signal retries.

### 4. Complete outage catch-up would change Slack from search to an event system

- **Requirement at risk:** Guarantee every gesture is processed after arbitrary
  downtime, including gestures removed before Phoenix observes them.
- **Why it looks inexpensive:** Resume from the last successful scan.
- **Hidden cost:** Reaction search has no reaction-add timestamp or retained event
  history. A broader message window still cannot reconstruct removed reactions.
- **Architectural consequences:** A durable event feed or relay, expanded access
  and a new capture/retention contract.
- **Blast radius:** Slack transport, baselining, state retention and acceptance
  semantics; more polling does not establish the guarantee.
- **Lower-cost alternative:** Resume the four-calendar-date window with retained
  identities and warn when older triggers may have been missed.
- **Disposition:** Excluded; the warning is a requirement, not a capture guarantee.

## Hidden Commitments

- Automatic recovery requires a continuing process-local owner even when no
  caller or UI is active. This is necessary background work, but not a new
  scheduler service. Removal/shutdown and explicit-action failures must stop or
  transfer it; an arbitrary attempt-count cutoff would defeat unattended recovery.
- A misclassified persistent error can cause continuing requests. Five-to-sixty
  second pacing, server deadlines and actionable status bound that cost. These
  timing values do not create new operator tuning controls.
- Existing credential/resource binding, rotation persistence, epoch fencing and
  one-shot no-replay guarantees are retained commitments. They are not replaced
  with a generic idempotency framework for arbitrary MCP operations.
- Native recovery diagnostics are part of the implementation scope. They do not
  promise persisted history, outage alert delivery or automatic production upgrades.

## Complexity Collapses

1. Process-local retries remove new timer/session tables and cross-version jobs.
2. Configured HTTP scope avoids new stdio respawn, removal and shutdown loops;
   existing transient-refresh behavior during removal stays intact.
3. Shared MCP ownership removes a separate gateway repair controller.
4. Connection repair does not replay tool calls, so it creates no general
   operation-deduplication or delivery-reconciliation subsystem.
5. Native status uses the current failure, retry time and existing scan evidence;
   no last-attempt database column or monitoring history is required.
6. Bounded catch-up and an honest warning avoid a new event-capture transport.

The remaining work—typed failure selection, one cancellable paced retry owner,
status projection and focused race tests—is necessary for the requested behavior.
It extends an existing lifecycle rather than introducing a new class of durable
system. Strict remote cleanup remains the consequential retained tradeoff.

## Validation

- Final repository Allium analysis passes without changing the accepted findings
  baseline. The touched Allium files also parse with zero structural errors.
- spEARS artifact shape and canonical requirement-anchor checks pass.
- The spec-shape wrapper also ran 1,279 deployment-tooling unit tests: 1,241
  passed, 37 skipped and one certificate-chain fixture errored under macOS
  LibreSSL. That exact test passes with OpenSSL 3. The full suite was not rerun;
  no fixture or implementation change was made for this environment difference.
- Whitespace validation passes. Only specification and review documents changed;
  the recovery acceptance scenarios above have not been implemented or executed.


## Owner-approved simplification and implementation

The owner accepted best-effort remote deletion and rejected durable connection
retry jobs, gateway reconnect/alert controllers and complete outage catch-up.
Only strict cleanup was an existing guarantee being removed; the latter three
were alternatives already excluded from the draft. ADR-230 records the decision.
Accepted Slack requests, conversations, bindings and reply obligations remain
durable. Local credential-removal intent still survives restart under ADR-086.

Implementation deletes cleanup-only refresh/sign-in/removal workflows and queued
configuration ownership. Scope/challenge plans now live on the existing supervisor,
independently of the retired transport. One process-local retry counter and deadline
cover configured connection and refresh recovery. Native status exposes state,
error and next retry; Slack projects it without another recovery controller.

### Deliberate regression retirement

The removed strict-cleanup cases covered: authenticated DELETE as a prerequisite;
cleanup challenges driving new sign-in or scope consent; deferred removal through
OAuth; configuration queued behind obsolete remote sessions; and repeated DELETE
attempts after refreshed grants. Those expectations conflict with the new contract.
The standalone repeated-DELETE test is subsumed by the single-attempt status matrix.
The obsolete UI pending-removal OAuth poll test is replaced by a background-recovery
polling test. Existing schema/epoch fencing, cancellation, OAuth nonce/issuer/scope
checks, grant rotation and one-shot accounting remain required regressions.

Replacement coverage includes the DELETE status matrix, removal without refreshing
expired credentials, live OAuth recovery despite failed DELETE, cancellation of a
scheduled retry on reconfiguration/removal, connection and token endpoint throttling,
local credential-removal intent surviving manager reconstruction, and an uncertain
one-shot operation whose connection recovers without replay. Startup refresh and
persistence regressions now allow the specified exponential delay rather than
assuming a constant five-second loop. Denied authorization retries preserve scope
plans after the old transport is gone.


### Implementation alignment and hardening pass

- **Code bug:** Authorization retry plans were stored on retained transports.
  Best-effort retirement exposed loss of the requested scope/challenge after a
  denied flow. The existing supervisor now retains the plan independently.
- **Code bug:** OAuth metadata failures were flattened into text. Typed discovery
  and grant failures now retain temporary/permanent classification and Retry-After.
  Invalid metadata stops automatic recovery without discarding a usable grant.
- **Code bug:** Failed credential invalidation could leave an old connection owner
  eligible to publish. Configuration changes now fence it before invalidation,
  retain the old configuration on failure and expose the error for explicit retry.
- **Code bug:** A reload-response timeout could stop a still-valid HTTP lifecycle.
  The response deadline now leaves the owned background operation running; each
  network operation retains its own bounded timeout.
- **Spec bug:** Two obsolete removal guards still referred to deferred refresh.
  They were removed; remote cleanup cannot defer removal into authorization.
- **Intentional abstraction:** Allium represents recovery phases behaviorally.
  Code retains the unfinished step through typed continuations and the retained
  refresh response; no stored phase history or operator phase UI is introduced.

### Local qualification

- Slack database regressions: 27 passed, including accepted-source deduplication
  across restart, exact shared-reply coverage, and single-attempt signal recovery.
- UI regressions: 16 passed. TypeScript and focused ESLint checks passed.
- Allium analysis and canonical requirement anchors passed. Direct spEARS artifact
  shape validation passed; deployment-tooling tests were not repeated for runtime
  changes. Formatting and whitespace checks passed.
- Full MCP engine suite: 180 passed. Application Slack regressions: 54 passed.
  Together with the database and UI regressions, 277 selected tests passed.
  Clippy passed for all targets of the MCP and application crates with warnings
  denied. Only pre-existing ts-rs macro attribute notices were emitted.
- No production deployment, prototype database conversion, live Slack fault
  injection, workflow modification or external message send was performed.


## PR code-review follow-up

The first GitHub Codex review found three implementation gaps: ordinary tool
throttling could reconnect and replay; readiness checked names without schemas;
and callers could outlive their admitted timeout during recovery. Tool 429/503
responses now retain Retry-After without reconnect or replay. Settings validate
the adapter's argument forms against cached schemas, report unsupported schemas
as unverified, and perform no remote I/O for catalog inspection. Ordinary calls
retain one configured budget across readiness, recovery and replay; timeout
cancels the invocation while owned recovery continues. A scheduled background
retry also returns current unavailability promptly.

An author review additionally reproduced credential-bearing endpoint URLs in
request errors. Request diagnostics now remove URLs before rendering errors.
Focused follow-up validation passes: 185 MCP tests, 26 UI tests, TypeScript,
focused ESLint and MCP Clippy. The application crate's all-target Clippy passed
after the cached-catalog API change; the later call-budget change remains internal
to the separately checked MCP crate. CI qualifies the combined public PR head.


The second GitHub review found initial OAuth metadata errors being flattened
before reaching connection retry selection, and temporary failures preventing
alternate metadata endpoints from being tried. Initial metadata errors now retain
the typed category, Retry-After and authorization context through connection
setup. Discovery tries distinct candidate URLs and aggregates temporary failures
only when no usable candidate succeeds. Regression coverage includes fresh setup
without a stored grant, throttled setup without an unowned sign-in URL, and both
metadata families across all six retryable HTTP status codes.
The complete MCP suite now passes 188 tests, with MCP all-target Clippy and
Allium/anchor checks passing. Client-registration/callback setup and failed owned
re-authorization retain their existing explicit-action boundary; this follow-up
does not introduce automatic identity-provisioning retries.

The third GitHub review found redirect errors being treated as network outages,
non-JSON token rejections preserving a dead grant, delayed replacement retries
carrying an unconditional sign-in decision, and the standalone Slack settings
page omitting the owned sign-in action. Redirect failures now require correction.
Definitive token endpoint rejection discards the grant even without a JSON body;
malformed successful responses remain permanent protocol errors and retain it.
Delayed retries compare expiry and scope/resource challenge evidence before
refreshing, while an unexpired grant rejected for the same challenge still asks
for sign-in. Callback reconnects retain the original challenge for that comparison.
Settings expose the existing pending authorization URL for enabled connections.

The scope-preservation regression now permits one refresh with the rotated token
when delayed recovery encounters a different scope challenge, then checks that
rejection still preserves the complete scope union for sign-in. No regression
was removed. All 192 MCP tests and 28 focused UI tests pass, along with MCP
all-target Clippy, TypeScript, focused ESLint, formatting and Allium/anchor checks.

CI found that an independently merged decision had taken ADR-229. After rebasing
onto current main, this unpublished decision is ADR-230; existing accepted
decisions are unchanged. The combined spec-shape validator passes locally.

The next GitHub review found idle GET-stream expiry deferred recovery until a
consumer read, malformed metadata errors exposing challenge URLs, and incomplete
HTTP-date parsing for Retry-After. Idle expiry now wakes the existing supervisor
recovery claim. The observer is scoped to the serving epoch/connection, exits on
retirement and holds neither the manager map nor supervisor mailbox alive.
Metadata diagnostics omit endpoint/challenge URLs, and Retry-After uses the
already locked HTTP-date parser for modern, RFC 850 and asctime date forms.
Regressions exercise autonomous idle recovery, manager/transport release,
malformed private metadata through the status API, and all three date formats.
All 194 MCP tests pass, with all-target MCP Clippy, formatting and Allium/anchor
checks passing. These changes leave the previously verified UI code unchanged.
