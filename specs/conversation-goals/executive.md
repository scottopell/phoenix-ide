# Durable Conversation Goals — Immutable Storage Foundation

## Bounded local typed-watch contract — docs first, implementation owed

REQ-GOAL-011–014, REQ-GR-015 and REQ-CEVT-006 now define the locked local
watch increment. ADR-184 records causal ordering and failure ownership. This is
a contract-only cut; no watch capture, fanout, schema, decoder or UI implementation
is included, and no watch execution result is claimed.

| Contract | Implementation/verification boundary |
|---|---|
| REQ-GOAL-011 | Existing terminal/wait outbox transactions must capture saved current goal or observed absence, distinct from legacy not-captured; usage explicitly unavailable. Storage and boundary tests owed. |
| REQ-GOAL-012 | Standalone events only for accepted settled target Create/State; no report/Stop duplicates, no goal-version dedup of execution events. Ownership/enrollment/retry matrix tests owed. |
| REQ-GOAL-013 | Capture/append errors follow the owning transaction, with no cross-transaction or Stop recovery promise. Failure-injection/checkpoint tests owed. |
| REQ-GOAL-014 | Event-owned immutable capture, no source-pinning FK, no historical reprojection. Coordinated forward migration and populated preservation/deletion tests owed. |
| REQ-CEVT-006 | Saved metadata through existing event wire/read paths, exact outcomes and privacy retained. Codegen, API and rendered tests owed. |

Standalone nonboundary-report notifications, remote goal-change fanout, usage
accounting and automatic follow-through are deferred, not completed features.
The cut does not install services, start another worker or activate normal
startup. Parent migration coordination reserved **135** for this bounded local
watch forward migration after checking the live federation stack and other
owners. **No registry entry or DDL is authored** in this docs cut. Reservation
is not import/stacking authority or execution evidence. Migrations 132/133/134
and all other owners' SQL remain untouched.

Preservation review must inventory the latest `question_wait_outbox.sql` plus
federation/retirement overlays, including wait/mandatory route constraints,
payload immutability, acceptance triggers, pending-source restrictions, all
three message/steering/turn inbound origins, delivery identities and retirement
dependencies. Tests must retain IDs, payloads, acceptance/continuation state and
source-delete behavior, and distinguish legacy absence without live backfill.

Docs use the existing locked transaction matrix; they add no Allium file or wire
variant. Source inspection and whitespace checks are not executed validation.
Spec-shape/Allium/anchor checks have not run on this docs cut; no local compiler,
test, SQL, browser, Cargo metadata, push or CI dispatch is included.

## Latest executed floor — ordinary UI and focused backend, not typed watch

The parent reported hosted run `39451089782` at exact
`efcc5edfe1372b2e627f67c6bac01e328480995e` terminal with a failed aggregate
result but green owned UI/specs and focused backend checks. UI lane
`118725946176` passed all 12 checks in 302.6 seconds: TypeScript 15.8, ESLint
8.6, stylelint 1.5 and Vitest 59.1 seconds. Parent-harness evidence is retained
under `evidence/goals-ci-39451089782/`, including `ui-specs.log`.

Rust artifact `12280786889` records **209/209 focused passed** in 247.845
seconds with 3384 skipped; capture took 226.3 seconds, codegen reuse 1.2 and
freshness passed. Broad hit its 1500-second timeout: 5179 of 5507 run, 5143
passed, 36 failed and 191 skipped in 1499.584 seconds. This partial result is
not a completed comparison or waiver of the earlier shared failures. Clippy
remains red with 13 library and 17 test findings; a visible tail without an
owned root is not a full-inventory-clear claim. These failures remain unwaived.

This qualifies the exact ordinary UI/backend floor, not new watch code, a browser
journey, activation, full-feature completion or production behavior. Normal
startup remains services-uninstalled and default OFF. All earlier exact-head
results below are historical evidence, not qualification of the new contract.

## Ordinary-page factual UI — implementation and historical repair evidence

The ordinary `ProductConversationPage` supplies goal summary/details through the
existing embedded conversation `stateBarExtension`. Coordinator/Global extension
selection remains unchanged. When no embedded live transcript exists, the same
facts appear in a collapsed details element; History and Close render no editable
command fields. Component CSS is colocated; `useConversationGoals` owns the
page-lifetime read/command fencing separately from presentation exports. No new
panel framework, chat message, watch worker or automatic-continuation integration
is included.

`goalApi.ts` validates the actual Rust wire encodings, including externally tagged
`GoalState`, `GoalActor`, `GoalChange` and report observations, `recorded_at_us`,
explicit command dispositions, and the control receipt's `principal_id` and
`expected_control_version`. Integers beyond JavaScript's safe range, malformed
payloads, wrong-owner facts and oversized/non-increasing history pages fail
closed rather than being coerced. No generated SSE files are hand-edited.

Current goal, control and first immutable-history page are read independently in
parallel. They are **not** presented as one atomic combined snapshot and are not
required to share a version. History pages advance from the last returned owner
version. Refresh is explicit; there is no goal polling/watch loop. Uninstalled
503 is shown as unavailable, not an empty goal or successful activation. Failed
reads disable fresh commands. Reading or editing an objective grants no execution
permission or consent.

Create/replace, explicit Underway/Blocked/Complete/Exhausted state commands, and
Stop require an explicit user action. State reasons are required where the
existing command DTO requires them. Commands use factual goal/control CAS values;
Stop uses zero only after a successful null control GET, matching the qualified
absent-row mutation contract. No Release or implicit Stop release is offered.
Successful/refused receipts cause factual refetch, not optimistic goal mutation.
Objective text is sent unchanged and rendered without summarization.

Each submitted command's UUID and payload remain fixed for unknown-outcome
retries. Network errors, 5xx other than the route's definite pre-mutation 503,
and invalid/mismatched success receipts block new commands and retain that exact
retry. Accepted receipts correlate both user-control actors and the exact Create
objective/identity or State disposition/reason in the mutation and resulting
snapshot. Typed refusal receipts and definite auth/admission/uninstalled HTTP
rejections remain distinct; uninstalled/admission JSON errors must match the
actual route contract, not an arbitrary 503/409 body. A pre-mutation rejection of
an exact retry cannot establish the original attempt's outcome: only a correlated
receipt clears that retained unknown command. Refresh failure and owner
switchback cannot discard it.
Read-generation and owner-epoch fences prevent stale reads, mutation receipts or
refetch completion from changing another owner's view, including switchback.
Pending commands are retained per canonical owner within the mounted page; no
durable browser-restart or remount recovery queue is claimed.

Authored tests cover Rust wire variants and unsafe/foreign/history rejection,
default 503 versus null absence, request/receipt correlation, no optimistic
updates, exact unknown retry after failed refresh or a rejected retry, owner
switchback/unmount fences, late control/history reads and mutation refetch,
independent CAS values, explicit reasons, null-to-zero Stop, bounded history
pagination and fail-closed page rejection, mounted ordinary extension controls,
read-only History and preservation of the supplied coordinator extension.

The parent reported terminal failure of the UI lane `118690151347` in hosted run
`39440857025` at exact `5f1203f858bb4c410574453b6c5efc0b5e8f9065`, retaining
`evidence/goals-ci-39440857025/ui-specs.log` in the parent harness. TypeScript and
ESLint failed; Vitest reported 3808 passed, 2 failed and 2 skipped across 182
files (181 passed). The two failed cases were the 403/503 unknown-retry tests.
The initial readiness-only diagnosis was incomplete: the keyed details component
also resets the new owner's objective to blank, correctly disabling Create.
Stylelint, ast-grep, manual-SQL audit, spec shape, Allium, dev.py and package-lock
checks passed in that lane. Static review missed these three owned roots; it did
not qualify UI execution.

The first correction, `75c78339a8960866830aa0e2c07a839bdcf11a97`, separated
unavailable/error discriminants in `Facts`, replaced the type-only unused
disposition schema with the equivalent explicit request DTO union, and added a
readiness wait in the retry tests. Parent-reported hosted run `39447059443`, UI
lane `118711877469`, passed TypeScript, ESLint and stylelint but failed Vitest:
3807 passed, 3 failed and 2 skipped across 182 files (180 passed). Both retry
cases still failed because the new owner's objective was empty. The third
failure was the finalization fixture's structural render count (8 versus 6).
The parent retains `evidence/goals-ci-39447059443/ui-specs.log`; the existing
run completed with a backend-only focused pass; it did not qualify the failed UI.

The second bounded correction changes fixtures, not production behavior. The
retry cases verify the new owner's empty objective, enter an unsent objective,
wait for enabled Create and assert that no additional command was sent before
switching back. The original owner's exact UUID/payload and blocking assertions
remain intact; owner keys and blank-objective rejection are unchanged.

Source inspection found that the finalization fixture mocked the main `api` but
not `goalApi`, and its `open()` waited only for transcript text. The real
`useConversationGoals.refresh` starts independent reads and later sets facts on
the product owner after `Promise.all`, allowing initial network completion to
cross the token-churn baseline. The fixture now supplies deterministic null/empty
goal reads and renders the real extension summary, then waits for `GOAL none`
(the committed ready state) before returning from `open()`. The structural,
embedded and history render-count assertions are unchanged, with additional
assertions that token/heartbeat churn starts no new goal reads or commands.
The isolation correction was source-grounded, then executed successfully at
exact efcc in the latest floor above. No token-driven production re-render
defect was established by the contaminated baseline. This fixture execution
is not a browser journey or activation claim. Typed watch and automatic
follow-through remain outside that executed floor.

An earlier parent-reported backend-only floor was **209/209 at exact
`5f1203f858bb4c410574453b6c5efc0b5e8f9065`**, run `39440857025`, artifact
`12275977275`, 257.256 seconds with 3384 skipped. Capture took 505.4 seconds,
codegen reuse 1.2 seconds and freshness passed. Broad hit its 1500-second timeout:
5184 of 5507 run, 5151 passed, 33 failed and 191 skipped in 1499.614 seconds.
This is a partial run, not a completed comparison to the earlier shared 30.
Clippy reported 13 library and 17 test findings; no owned root in the visible
tail is not a full-inventory-clear claim. These failures remain unwaived and
backend-only qualification does not qualify UI.

The earlier backend floor was **209/209 at exact
`245da1764bf1085dc25d6dff29e3b8b79114de34`**, run `39431777571`, artifact
`12268869127`, 145.406 seconds with captured full-workspace harnesses. All three
control-read router tests and typed-admission unit passed there; capture took
171.1 seconds, codegen reuse 1.0 seconds, and stale/budget/UI-specs/e2e passed.
Broad completed with 5507 run, 5477 passed, 30 failed and 191 skipped in 908.274
seconds. All 30 failures and shared Clippy findings remain unwaived; absence of
owned roots in a retained Clippy tail is not a full-inventory-clear claim.
Normal startup remains services-uninstalled and default OFF. No schema SQL,
installer activation or production deployment is included. The dated sections
below are older checkpoint evidence, not qualification of this UI cut.

## Private authenticated install seam — source cut

The API has a crate-private `goal_control::private_install` seam that constructs
command/control services only when the supplied runtime's existing database
handle is already `GoalControlInstallation::Installed`. It does not change that
flag, install migrations, read an environment switch, or attach itself to normal
startup. Default services retain `None`; the normal router merges only the
uninstalled routes. Authenticated, valid requests to those uninstalled routes
return 503. No public installer or production activation is included.

All adapters extract the existing `OwnerAuthenticated` capability before
accessing services. Installed-router tests use the real authentication
middleware, not a trusted request header. Create/state bodies reuse the existing
`deny_unknown_fields` DTOs; attribution is derived by the existing command
services from the host instance identity, not caller-supplied actor data.

| Method / stable-owner path | Result / delegated authority |
|---|---|
| GET `/api/product-conversations/:reference/goal` | Current immutable `GoalHistoryEntry`, or JSON null when no goal exists; goal ID, objective, version, state/reason and persisted attribution remain factual |
| GET `/api/product-conversations/:reference/goal/history?after=<GoalVersion>` | Immutable changes ordered by owner version, at most 100, strictly after the typed cursor |
| POST `/api/product-conversations/:reference/goal` | Existing `GoalCommandService::create`, including host identity, exact request replay, settled/latest/Close fencing and Stop preservation |
| POST `/api/product-conversations/:reference/goal/state` | Existing `GoalCommandService::state`; explicit user disposition, never completion inferred from execution state |
| POST `/api/product-conversations/:reference/goal-control` | Existing `GoalControlService::apply` and its unchanged typed receipt/status mapping |
| GET `/api/product-conversations/:reference/goal-control` | Source-only prerequisite for truthful Stop UI: existing `ControlSnapshot` (`version`, `stopped`), or null only when no control row exists |

The factual control GET is an unqualified backend prerequisite cut, separate
from UI wiring. It uses `OwnerAuthenticated`, default-uninstalled 503 behavior,
and canonical ordinary owner resolution plus control lookup on one read
transaction. It creates no control row, epoch, receipt, chat or consent. The
existing snapshot type contains **only** nonzero `version` and `stopped`;
disarmed/consent fields are not fabricated. A null read means actual absence,
not consent or authorization. The existing mutation implementation explicitly
uses expected CAS version zero for absent control storage; an authored router
regression reads null without writes, submits Stop at zero, observes version
one/stopped, releases at one, and verifies version two plus replay/stale CAS.
Two further authored tests cover real auth/peer/alias/coordinator rejection and
genuine continuation/live-Close factual reads with unchanged write fences.
The default-OFF test also includes control GET. These new tests have not run.
Completed-History retirement remains outside executed coverage. Normal startup
remains uninstalled, and no UI/watch/autocontinuation is included in this cut.

Factual reads resolve the canonical ordinary ProductConversation via the existing
`get_ordinary_product_conversation_on` and read facts on **the same SQLite read
transaction**. They do not resolve arbitrary transcript/peer aliases, create a
goal/control row, record a command receipt, or append chat. Current-entry reads
validate the exact current immutable snapshot/history match. History uses the
same bounded SQL/decoder as the existing raw history reader; existing raw-reader
fixture semantics are unchanged. Reads deliberately do not use mutation/Close
admission or infer goal completion from conversation state.

Six `conversation_goal_private_install_*` router tests cover default-OFF/auth
ordering/all-or-nothing installation, guest/peer/header/DTO spoof rejection and
canonical/coordinator rejection, host attribution and exact replay, Stop
preservation and factual blocker reasons, genuine continuation and live-Close
read/write fences, settled product/acceptance gates, and bounded typed history
pagination with no read-side rows/chat. All six and the typed admission-mapper
unit test **passed at exact `022ec5fd02e423614ed305f3bc97e01f30a6b36d`** in the
hosted focused qualification described below. Completed-History read eligibility is source-only:
the canonical read path admits its lifecycle, but the new tests do not execute
full Close retirement. No fake Close phase/ledger fixture substitutes for that
coverage. A separate authored mapper unit regression covers both direct-storage
and nested goal-admission `ProductConversationUnavailable`/`CloseAdmissionFenced`
errors as HTTP 409; it is not a completed-History lifecycle journey. The child
fixture uses the existing test-support `create_conversation` API with its actual
parent ID, preserving inherited ProductConversation identity and transcript-alias
rejection without an invented subagent constructor. No UI, watch/outbox delivery or normal-startup installation is added.

The latest hosted focused receipt is **205/205 at exact
`d398e4fe95041ffd1571ca063d8ba22aff70058f`** with captured full-workspace harnesses
(run `39425602545`, artifact `12267966788`, 248.454 seconds, 3384 skipped).
Capture took 206.3 seconds, codegen reuse 1.1 seconds, and the codegen-stale
check passed. Command-budget, UI/specs, and e2e passed. The two owned router
Clippy roots were absent from the retained tail; that is not a full-inventory
Clippy-clear claim. This supersedes the 205-test floor at `022ec` without
implying isolated-three-package build equivalence or qualifying later edits.

Overall CI remained red. Broad execution hit the 1500-second budget: 5257/5503
ran, 5224 passed, 33 failed, and 191 skipped in 1499.624 seconds. These are
partial results, not a completed comparison with the prior 30-failure run.
Shared broad failures and Clippy findings remain unwaived. Production activation
remains OFF. The dated paragraphs below retain earlier
checkpoint history, not newer qualification claims.

## Latest actual134 qualification — exact778 SQL-safe-string compile failure

Parent run **39394922634**, exact
`778604eac663dec4710f50a0b45a1430ecb5a368`, is **terminal failed**, artifact
**12251990910**, with **zero focused tests executed**. Module resolution and
formatting passed the earlier stop; SQLx0.9 then rejected the registered fixture's
`query_scalar(&format!(...))` because `&String` is not `SqlSafeStr`. Source replaces
that three-name dynamic query with three literal SQL strings paired with their
table labels, preserving all count-zero assertions. The actual SQLx declaration
implements the contract for `&'static str`; no `AssertSqlSafe`, unchecked escape,
assertion removal or production query/schema change is introduced. No additional
formatted SQL query remains in the registration upgrade test module. Prior source
review missed this API constraint; actual compiler evidence takes precedence.

Completed Clippy118535304638 was inspected. Its retained diagnostic tail exposes
shared federation/global-read/send-chat roots with12lib/15lib-test summaries and
no new goal-owned root in that tail; it is not full-inventory clearance. UI/specs
and E2E fully passed per parent, including the named-registry regressions. The
next correction is source-only: standalone rustfmt and whitespace/API inspection,
no compiler/tests/SQL/browser, new CI, push, activation or resource changes.
185/185 remains qualified **only at d4c**;134 is registered but unqualified,
activation remains off, schema132 unchanged and11stashes preserved.

## Latest actual134 qualification — exact91 module resolution and registry grammar failures

Parent run **39393190783**, exact
`91ff9f77bde08c3be5ac8e1730458fdf9732828c`, is **terminal failed**, artifact
**12251126226**, with **zero focused tests executed**. Rust E0583 and Cargo fmt
could not resolve the new `registered` child from its path-attributed parent.
The source correction explicitly points to `user_control_upgrade/registered.rs`.
Standalone rustfmt traverses that child successfully after the correction; this
is module-resolution/format evidence, not compilation or test execution. Child
imports were audited against explicit goal-domain imports, root fixture imports,
and the parent chain's Database/message/checkpoint/report/owner/replay helpers.
The earlier bounded review missed the module path; compiler evidence supersedes it.

The completed Clippy118529529748 and UI118529529732 logs were inspected. E2E
passed, but UI/specs **did not fully pass**:1098 dev.py unit tests had2failures,
2errors and1skip. All four roots reject the actual candidate migration registry.
The new inline134 `concat!` expression is outside both deployment helpers' existing
strict grammar. The correction names the same expression `MIGRATION_134` outside
the registry and uses `sql: MIGRATION_134` inside, matching existing supported
constant entries. Control SQL, newline and user-control SQL remain identical;
no executable SQL, ledger identity, parser grammar, protected hash policy or
qualified-target128 fence is changed. Shared Clippy12lib/15lib-test summaries
remain unwaived; no hidden inventory is declared clean.

Two authored Python regressions check the actual134 registry under ordinary
parsing and preserve protected-target rejection: launchd uses
`qualified_current_target=False` for exact134-name/full-ledger inspection, then
requires the ordinary default protected qualification to reject newer targets;
systemd checks ordinary parsing and target128 rejection even with the fixture's
reviewed hash matched. No helper policy or production deployment is changed.

This is a coherent source correction, not newly qualified134. Only standalone
rustfmt, whitespace, source/API/import review and immutable-file comparisons ran;
no compiler/tests/SQL/browser, push, new CI, activation or resource expansion.
185/185 remains the focused floor **only at d4c**. Eleven stashes are preserved.

## Independent ca49 review corrections — exact FK timing and migrated fixture inventory

Review found two pre-execution blockers in the134 registration cut. The actor
probe used nonexistent columns and misclassified its receipt FK as deferred.
It now inserts a valid UserControl actor shape using `control_principal_id`,
`control_request_id`, `control_command` and `control_result_version`, leaving all
source fields NULL. The missing receipt must fail **at INSERT**, classified as a
foreign-key violation, followed by explicit transaction rollback. A separate
syntactically valid accepted state receipt for existing goal/version1 points to
missing next history/version2: INSERT must succeed once and **COMMIT** must fail
with a foreign-key violation. The probe receipt must then be absent. This tests
the actual immediate actor-to-receipt and deferred receipt-to-history contracts;
no SQL/schema semantics are changed. Health checks, retained-row equality and
subsequent changed-intent rejection remain present.

The runtime control-service fixture opens a fully migrated database. It now
verifies all three registered control/request tables instead of applying duplicate
candidate DDL. A repository-wide Rust include/caller inventory leaves candidate
SQL only at: the registered migration concat; the minimal hand-built substrate
in `tests/control_candidate.rs`; the explicitly pre134, installed-control branch
of `admitted_fixture_with_control_at`; and `user_control_upgrade`'s candidate-only
rebuild tests over that genuine pre134 fixture. Those older substrates still
require their explicit DDL; they are not silently migrated or skipped.

These are source-review corrections, not execution evidence. Only standalone
formatting/whitespace and bounded source/API checks ran; no compiler/tests/SQL,
browser, push or CI.185/185 remains qualified **only at d4c**. Activation/public
installation remain off; schema132 and executable134 SQL are unchanged, and all
11 stashes are preserved.

## Qualification floor and next source cut — migration134 registered, activation still off

Parent run **39386564244**, exact
`d4c306dc9d315f719ea7a1eda0deb13722c65c6b`, is terminal failed overall but establishes
**185/185 focused PASS** in169.521s with3393skipped, artifact **12249700355**.
This is the new full focused floor **only at d4c**, not at later source commits.
The final canonical cancellation payload, later `Cancelled`/owns0 and provider/tool
count assertions executed and passed; actual133 migration tests are included.
Compile190.9s, codegen133.2s and generated-stale guard passed. Broad Rust completed
5492run/5462passed/30failed/191skipped in1180.095s. UI/specs and E2E passed; shared
Clippy and the30 broad failures remain blockers, not waived by focused success.

Following that prerequisite, the next source-only slice registers134 as
`durable_conversation_goal_user_control`: its SQL is exactly the existing control
candidate followed by the existing user-control rebuild. The only candidate-file
change is its obsolete unregistered comment; executable SQL and132 remain unchanged.
The ordinary migration runner supplies the atomic transaction and ledger write;
no new migration engine or compatibility/recovery tier is introduced.

Registration is **schema only**, not runtime installation. `Database` still starts
with `GoalControlInstallation::Disabled`, goal/report/management services and
command/control services retain their disabled defaults, and public control stays
unavailable. No production installer, authenticated read route, UI mount, goal
watch emitter or automatic-continuation policy is added. Those remain separate
commissioned work and are not delivered by this cut. The test-only control helper
now verifies migrated table presence and flips only the isolated test instance;
fresh management/user-command fixtures no longer run candidate DDL a second time.

Existing candidate rebuild/FK/immutability/cycle tests remain intact but are fed
real pre134 databases, with control-candidate installation explicit only where
those older candidate tests require it. A test-only runner bound ends at133; it
uses the existing real migration list, not fabricated version stamps. Four new
registered-runner regression functions cover fresh/real133 schema parity and
inert defaults; populated report-history/binding cycles with ordinary, Anthropic
and Responses checkpoint owners plus exact retained checkpoint replay; injected
mid-cycle delete failure rolling back schema/data/source/ledger then successful
retry; and exact user-command receipt replay with changed-intent rejection.
Assertions include foreign keys enabled, FK/integrity checks, empty staging,
unchanged history/actors/objectives/bindings, messages/state/turn ownership and
typed replay side rows, supported hard-delete cleanup, idempotent migration and
no manufactured control/user requests. Provider-side fixtures use the existing
owner replay builders; this is not a new signature/provider-dispatch E2E claim.

The older131 upgrade explicitly covers132through134 with all three exact ledger
names. The remote130 fixture defers134 as well, removes all three later stamps,
actually applies132+133+134 and still checks133 against the fresh schema. The133
notes test queries133 explicitly rather than misidentifying it as latest; ledger
ordering/capability tests include134. No prior regression is removed.

Only standalone formatting, whitespace, source/API review and immutable-file
comparison are performed locally. No compiler/tests, SQL execution, browser,
new CI, push, merge, deploy or resource expansion. These134 tests are **authored,
not executed**;185/185 remains evidence only for d4c. Eleven stashes are preserved.

## Latest actual qualification — exact239184/185, cancellation expectation corrected

Parent run **39383636793**, exact
`239cbbc56aed18d86395f5aac7635d26423fca92`, is **terminal failed**, artifact
**12247587393**. Focused185executed:184passed,1failed,3393skipped in117.498s.
All six earlier focused failures passed here, including signed provider replay,
rollback, source authority, candidate user-state installation and real132+133
migration tests. This qualifies those specific test results at239, not activation
or a fully green focused floor.156/156 at e4d remains the last full focused green.
Broad Rust completed:5492run,5461passed,31failed,191skipped in846.453s; it is not
the earlier timeout. UI/specs and E2E fully passed. Shared Clippy remains red with
12lib/15lib-test error summaries in the retained tail; those blockers are unwaived.

The sole focused failure is the new management cancellation test expecting
`is_error=true`. The actual ToolAborted transition calls
`build_cancellation_results` with `Cancelled by user`; for `goal_manage` it uses
`ToolResult::cancelled`, whose typed outcome is `ToolOutcome::Cancelled` and whose
persisted content is intentionally **non-error**. The distinct goal-report error
special-case does not apply to management. No production cancellation policy is
changed. The source correction asserts the exact cancellation outcome/message,
`is_error=false`, and full equality with the canonical builder's persisted tool
content, including call ID and images. It retains the one-result count and every
terminal/no-mutation/no-replay/no-dispatch assertion.

At239 the failed test already established Idle, no active turn, no pending/DB
private replay, no target mutation and one canonical result before failing. The
later `Cancelled`/owns_conversation0 SQL assertion and final provider/tool count
assertions were **not reached**; they remain in place for the next qualification,
not retrospectively claimed passed. This correction is source-only, with
standalone formatting/whitespace checks and no local compiler/tests/SQL/browser,
push, new CI, feature expansion or activation. Eleven stashes and immutable132
remain intact;134 is unregistered/default-off.

## Latest actual qualification — exact07 production import failure

Parent run **39382847878**, exact
`07cfc5987641f2423a82e568fde04c22d3474ef3`, is **terminal failed**, artifact
**12245899259**, with **zero focused tests executed**. Two production E0425
errors at the cancellation/canonical-management helper signatures reference
`AssistantMessage` without a module-scope import. Existing nested test imports
do not supply production scope. The standalone source correction adds that type
to the existing `crate::state_machine::state` import; the re-export resolves to
`phoenix_core::domain::sm_state::AssistantMessage`. No helper behavior or fixture
assertion changes. Prior static review missed this import; compiler evidence wins.

The completed Clippy118495802967 and UI/specs118495803149 job logs were inspected.
Rust, Clippy, E2E and the UI lane's dev.py unit check encountered the same two
production compile errors. Individual UI TypeScript/timing checks passing does
**not** make UI/specs fully successful at this HEAD. No hidden lint inventory is
claimed cleared. Newly added helper signatures and body type paths were checked
against module imports, local imports and fully qualified declarations; this is
bounded source review, not compiler/typecheck evidence. Only standalone rustfmt
and Git whitespace checks ran locally, with no compiler/tests/SQL/browser,
push, new CI, feature expansion or activation. Eleven stashes, immutable132,
unregistered/default-off134 and the156onlye4d full floor are unchanged;133 is not
fully qualified and ade178/184 remains partial evidence.

## Independent72e review correction — missing active authority fails before replay take

Review of exact72e found an owned fail-closed gap: the installed-service path
combined absent service and absent active turn as `Ok(false)`, permitting the
state-progress writer to consume signed replay before tool dispatch rejected the
missing turn. Source correction keeps absent service and ineligible role behavior
unchanged but recognizes a canonical typed management candidate/pending intent
before requiring the active turn. Missing authority now returns an error before
`take_state_progress_replay` can take or persist the payload. Stale authority
continues through the existing host-service validation error; tool name alone
still cannot defer replay. Both current/remaining call and typed pending branches
use this ordering, including their cancelling-state callers.

The negative provider fixture retains all existing public-message/goal/dispatch
assertions, captures the exact typed signed replay from its real outcome and
checks it remains pending with no persisted Anthropic or Responses replay on
missing or stale active authority. Missing-turn errors are checked explicitly.
The ordinary-role branch is not forced into Coordinator replay semantics. These
are source-only assertions, not executed results. No CI, push, activation,
registration, compiler/tests/SQL/browser or resource changes;11stashes preserved.
The full focused floor remains156onlye4d;178/184 atade is partial evidence.

## Latest actual qualification — exactade178/184 is a partial result, not a floor

Parent run **39377097203**, exact
`ade9ea18ab6fb5181e0f4b58a1c9a8b4a44cf169`, is **terminal failed**, artifact
**12245117981**. Focused qualification executed184:178passed,6failed,3393skipped
in222.530s. Compile223.9s, codegen146.8s and generated-stale guard passed. The new
shared-future16KiB/unpolled-drop regression and runtime management owner/replay
retry/recovery tests passed at this HEAD. That is bounded actual evidence, not a
new fully green floor;156/156 at exacte4d remains the last full focused green.
Broad Rust timed out at1500s with a partial5491-test plan:4940executed,
4905passed,35failed,191skipped. It is not a completed full-suite result.
UI/specs fully succeeded per parent. Shared Clippy blockers remain unwaived;
only the visible goal-owned request-guard issue is source-corrected in922.

The six focused failures have four traced setup/behavior roots:

- The prior131 upgrade really applies132 **and**133. Its test now names both,
  expects two applied migrations, checks both exact ledger names, asserts133's
  notes column shape, and keeps preserved source rows, empty132 goal storage,
  rerun idempotence and legacy UserApi goal-history checks. It does not change
  either migration or pretend to isolate132 by merely changing a count.
- Both ordinary-source rejection fixtures failed while materializing their own
  canonical turn. Test setup now uses actual `apply_goal_control(Release)` with
  the saved current version before authoritative submission; rejection assertions
  remain at the management service/provider boundaries, not an earlier setup error.
- The user-state provider fixture needs normalized user-request tables. Its
  isolated test database now installs `user_control_candidate.sql` in a transaction,
  following the existing user-command fixture pattern.134 remains unregistered;
  this is not production installation or activation.
- Both signed provider journeys exposed premature private replay persistence:
  `RenderingAnthropicClient` returns signed metadata, `process_outcome` captures
  it with the canonical owner, then the core ToolExecuting transition emits
  `PersistState` before `ExecuteTool`. That state-progress write took the pending
  replay before the canonical management transaction. Assertions were not weakened.

The source correction leaves intermediate state writes intact but defers private
replay consumption while the actual host-installed management service validates
active Coordinator turn/generation and canonical assistant/tool/request ownership.
Current and remaining calls are parsed to the typed request, then host-verified;
pending typed intents are reverified and compared with the captured source.
CancellingTool recovers its current call by canonical ID before the same typed
host check, and covers skipped/pending calls too. A name string or request DTO
alone never authorizes deferral. Missing service/role or non-management rounds
keep the existing state-progress replay behavior. No dispatch or new durable
owner is introduced; the validation future is heap-pinned and immediately awaited
at the existing owner boundary, with the same borrowed lifecycle.

The original provider barriers now compare the exact typed signed replay captured
from the real outcome, require no DB private rows/public assistant/result or
changed goal/history before checkpoint, preserve the exact payload after a real
checkpoint rollback, and still demand signed second-request wire plus canonical
result after successful commit. An authored cancellation regression retains private
replay through the actual UserCancel/CancellingTool transition and supplies the
ToolAborted completion to the existing terminal checkpoint, asserting one canonical
cancellation result, cancelled ownership release, no goal mutation and no stranded replay.
This is a deterministic event-boundary test, not an OS/tool-task cancellation race
or restart-recovery proof. Existing terminal/recovery owner coverage remains in
scope; no new restart-replay policy is claimed.

This correction packet includes922's lexical guard fix and has only source-format,
whitespace and bounded source review locally. No compiler/tests/SQL/browser,
push, new CI or activation. Eleven stashes and immutable132 are preserved;
133 remains not fully focused-qualified and134 unregistered/default-off.

## Interim exactade Clippy observation — superseded by terminal result above

Before Rust terminated, parent reported run39377097203 at exacteade still in
progress; this interim observation made no focused outcome or new-floor claim. Completed Clippy job118477650972 has
lib12/lib-test16 error summaries in its diagnostic tail. The prior twelve visible
large-future and three Option-loop reports are absent from that tail, not blanket
clearance of hidden diagnostics or execution of the future-size regression.
One visible goal-owned root is `await_holding_lock` in the capability-absence
fixture. Its source correction encloses the captured-request count/declaration
assertions in a lexical block ending before the awaited no-target-mutation check.
All assertions are preserved; no clone, async mutex, explicit-drop workaround or
lint suppression is added. Neighboring request/wire assertion helpers are
synchronous and do not hold their guards across await. The completed job log was
read directly, without polling the running Rust lane. Shared federation/global/
send-chat diagnostics remain unwaived. UI/specs succeeded per parent. Only
formatting/whitespace/source checks ran locally; no compiler/tests/SQL/browser,
push, dispatch, activation or schema changes. Eleven stashes remain intact;
133 remains unqualified,134 unregistered/default-off,156floor onlye4d.

## Latest actual qualification — exact282 stopped on Option-loop warnings

Parent-owned run **39375980176**, exact
`2820a2b7d76e1f76038a23f6f6766ec88023cb1d`, is **terminal failed**, artifact
**12243672253**, with **zero focused tests executed**. The prior `ToolContent`
E0308 is absent. Three management provider fixtures iterate over the actual
`Option<Event>` from `execute_effect`; source changes use `if let Some(event)`
and preserve the zero-or-one `process_event` behavior without suppressions.
Parent reports UI/specs lanes fully successful. Migration133 is still unqualified;
134 remains unregistered/default-off. The actual focused floor is156/156 onlye4d.

Clippy job **118474205637** was read in full, but its 1883-line GitHub job log
contains only the check runner's diagnostic tail. It exposes **twelve** repeated
large-future reports at18544bytes and the three Option-loop warnings, followed
by a **summary** of70 lib-test errors. It is not a complete70-diagnostic inventory.
No omitted roots are declared cleared, waived or unrelated from that summary.
Shared federation/global-read/send-chat blockers remain unwaived.

The source correction heap-pins only the management checkpoint branch at the
shared `persist_pending_goal_round` dispatch and immediately awaits it. The
branch no longer contributes its entire nested service future inline to every
ordinary effect frame. No task is spawned or detached, references do not become
static, and admission ownership, mutable borrows, guard scopes and error handling
stay with the same awaited caller. Dropping that caller drops the boxed future;
there is no independent work owner. This is a source-level containment rationale,
**not** an executed measurement or cancellation-race result. No per-test boxing,
new lint allows, stack increase, worker increase or resource expansion is added.

One authored regression measures `size_of_val` on the actual unpolled shared
effect future against16KiB, then drops it and checks no durable/provider/tool work
occurred. This checks a future-frame budget and unpolled-drop behavior when run;
it does not exercise cancellation during SQL or prove threshold success now.
Existing checkpoint owner/replay tests remain the behavior checks. Only source
formatting and whitespace checks were performed locally; no compiler, tests,
SQL, browser, activation, push or new CI. The packet includes the prior9f
free-versus-held gate coverage refinement and awaits independent exact-HEAD review.

## Post282 source coverage refinement — free and held target gates

Parent captured exact282 for run **39375980176** and owns its monitoring and
artifact; its later actual failure is recorded above. The runtime exact-retry matrix now checks
each of its twelve cases twice: first after proving both target gates available
and releasing both, then with both held. This specifically closes the earlier
retained-refusal limitation: retry must not turn a saved refusal into a new
success when admission becomes possible. Each stage retains the exact saved
message publications, typed provider data, goal history, proposal, source state,
turn ownership and watcher assertions. These remain authored, unexecuted tests.
Negative cases assert **no Message publication**, not total SSE silence; provider
reads compare **typed private data**, not raw DB encoding. No production source,
UI, watch routing, autocontinuation, activation or migration registry is changed.

## Latest actual qualification — exact737 failed before focused execution

Parent-owned run **39374941681**, exact
`7372d588c9f2d08099df2cd297022d6ed8a6335a`, is **terminal failed**, artifact
**12242414430**, with **zero focused tests executed**. The six earlier provider
fixture compiler roots and two DB Clippy roots are absent from this run. The new
compile stop is the recovery fixture passing a `ToolResult` to
`MessageContent::Tool`, whose payload is `ToolContent`. The source correction
uses the actual `ToolResult::persisted_tool_content()` constructor, not an
invented conversion or a changed expected type. The earlier bounded static
review missed this error; actual compiler evidence supersedes it.

Completed Clippy job **118470954462** reached goal-owned IDE diagnostics: three
reports for imports after statements, a wildcard over `ToolRoundResult`, and
`persist_pending_goal_round` length106/100. Source corrections move imports to
module scope, explicitly enumerate finalized/report alternatives, and extract
the existing sequence reconciliation into its small shared helper before the
management/report split. No new lint suppressions are added. The full completed
Rust/Clippy logs were read, not rerun. Shared federation/global-read/send-chat
Clippy failures remain unwaived; neither IDE Clippy nor the new packet is claimed
green. Parent reports UI/specs lanes fully successful. The only actual focused
green floor remains156/156 at exacte4d; migration133 remains unqualified and134
unregistered. No new CI, push, activation, SQL or local compile/test was performed.

## Runtime exact-retry source correction — not executed

After parent capture of exact737 for run **39374941681**, source tracing found
management checkpoints branching before the retained assistant sequence
reconciliation already used by report checkpoints. Runtime checkpoint effects
reserve a fresh sequence range on each attempt, while exact retained-message
comparison includes `sequence_id`; DB/service fixtures retaining the original
sequence do not exercise that seam. The source fix extracts the existing
sequence-only reconciliation into a helper called before the management/report
split. All other
assistant payload, owner, result, proposal and private-replay comparisons remain
at their existing transaction owners. This fixes an existing exact-retry contract;
it does not add automatic restart replay or another recovery/compatibility tier.

Two additional source-only runtime regression functions cover twelve
ordinary/Fork effect-owner × absent/Anthropic/Responses replay × accepted/refused
cases, and two changed-assistant negative cases. They invoke the real
`apply_transition_result` orchestration with `PersistCheckpoint` or
`PersistForkProposal`, genuine application-gated pending management and real DB
owners. They assert initial saved message/publication equality and private bytes,
new sequence reservation on retry with exact retained rows and publications,
unchanged proposal/history/turn/state/replay snapshots, current durable source
state/timestamp and watcher installation, stable success/refusal with both target
gates held, consumed pending replay, and no new LLM/tool dispatch. Changed
assistant content must still fail closed without publishing or changing durable
state. Completed awaited orchestration is the synchronization boundary; channel
lag/closure is an assertion failure, not silently treated as no publication.

This is runtime checkpoint/effect coverage, **not** execution of `propose_task`,
a provider-dispatch E2E or browser/watch UI. No task file is created. The source
fix and tests have not been compiled or executed. Parent owns all run39374941681
results/artifacts; its actual terminal failure is recorded above. Schema132, migration
registration, generated types and activation remain unchanged, and broader
UI/watch/autocontinuation delivery remains incomplete.

## Latest source checkpoint — qualification remains parent-owned

Parent-owned run **39373782924**, exact
`e6b717f4f05040f69c347cbbdc29c7591b07a56e`, is **terminal failed**, with artifact
**12241829258**. Both broad Rust compilation and focused qualification stopped
on six compile errors; **zero focused tests executed**. The focused green floor
remains **156/156 at exact `e4d0d615061a0d927cc5ad3d1a3d875c7c375754`** only.
The parent reports the UI and specs lanes fully successful after the timing
barrier corrections; that does not qualify goal SQL or migration133.

The follow-up source correction uses the actual
`workflow::PersistedConversationProjection` and removes five writes to a
nonexistent LLM-generation field. Every received outcome must match the actual
runtime generation; four success paths go through
`process_generation_tagged_llm_outcome`, including its task/metrics cleanup. The
negative source-authority case asserts a current generation and directly checks
the `process_outcome` error, because the tagged loop boundary logs and consumes
that error instead of returning it. No fixture overwrites a request generation.

The same run's Clippy job **118467336375** stopped at exactly two DB management
roots: unnested admission or-patterns and a future-variant wildcard in canonical
tool-call lookup. Their source correction nests the admission alternatives and
exhaustively names all thirteen `ContentBlock` variants, preserving the guarded
`goal_manage` lookup. IDE Clippy roots were not reached and are not declared
clean. These fixes are source-only, not a new compilation or test result. The
writer fetched the completed Rust/Clippy logs read-only; no CI was dispatched.
The uncommitted UI presentation scaffold was withdrawn when these real failures
arrived; no UI leaf, route, watch integration or autocontinuation is claimed.

The later management coverage slice extracts the existing error reconciliation
branch into a private function without changing its classification or admission
policy. Three source-only regression functions cover: all Runtime/Fork/terminal
owners with Anthropic/Responses replay and accepted/refused outcomes; a missing
retained result pair; and actual Fork proposal/state/provider-row write failures.
Tests commit through the real application gates and DB transaction owners, then
invoke the same private reconciliation branch with a lost-response diagnostic.
This models lost acknowledgement at that branch; it is not fault injection into
the transport/commit call, automatic restart recovery, or a runtime Fork journey.
The Fork fixture pairs its proposal call/result with the canonical management
call; it does not execute the proposal tool or create a task on disk.

Assertions include full history/message/proposal/private-replay/state/turn
snapshots, exact output reuse, successful terminal ownership release and replay
cleanup, unchanged nonterminal source ownership, stable refusal after target eligibility
returns, historical success after target retirement with both target gates held,
and changed proposal rejection. Source formatting and whitespace checks are not
compiler, test, SQL or migration qualification. UI/watch/automatic continuation
remain commissioned and incomplete. Schema132 is immutable,134 unregistered,
production installation disabled, emitter absent and public control route503.

## Current qualification boundary

Parent-owned run **39349884955**, exact **`2ec904a6720140469eb840b0607302c64dc9da24`**,
is terminal FAILED overall. Downloaded Rust job118391408991 artifact12234391373
proves focused execution: **127 tests,127 passed,3391 filtered/skipped,119.396s**.
All five e19 failure roots individually pass, including the actual prepared-report
Stop race and terminal-response-loss reconstruction. This is the new wholly green
focused floor, superseding f6's96/96; it qualifies neither later369 source nor the
UserControl candidate.

Normal Rust checks compiled then exceeded their configured1500s budget:
5115/5429 tests executed,5080 passed,35 failed,191 skipped;314 did not run after
termination. Those35 assertions are not blanket-classified as upstream. Clippy
and UI/specs also failed; E2E, geometry, tasks and worker-budget lanes passed.
Two owned fixture Clippy lints are source-fixed in369, outside tested2ec.

The later exact4e run **39354314956** is terminal FAILED. Its Rust artifact
**12237890605** reports **140 focused tests:139 passed,1 failed,3434 skipped**
(126.507s). The owned failure is the warmed legacy actor decoder during the
candidate rebuild: SQLx cached `SELECT *` metadata retained ten columns after the
actor table gained six, panicking the worker and surfacing RowNotFound. Source
now uses an explicit legacy projection and a control-variant-only field query.
This partial result does **not** supersede the wholly passing exact2ec floor.
Broad Rust, clippy and UI/specs are red; shared failures are not waived. One
owned non-exhaustive control-error match is source-corrected here.

Exact87b499e963bfc70c1727ac2086db63a484951e87 run **39358832109** also
completed FAILED. Rust artifact **12236746272** contains two owned fixture
compile failures in both normal and focused stages: `LlmRequestMetricsRow`
requires `metrics.outcome`, and the in-process provider test named a nonexistent
`LlmTransport::Http` variant. **Zero focused tests executed at exact87.** These
fixture APIs are source-corrected after87; the partial exact4e result and wholly
green exact2ec floor remain distinct, and neither qualifies the corrected source.

Later exact3877baedcc1a72c2b3ac6327403f6a4361606f49 run **39361606004**
completed FAILED. Rust artifact **12237862532** proves **zero focused tests
executed**: the command fixture imported `TranscriptConversationId` from the
wrong module. The actual type is `domain::close::TranscriptConversationId`;
that fixture import and its statement-order imports are source-corrected.
The prior87 metrics/transport compiler roots are absent in this run.
Its Clippy job **118437207992** reported one library plus sixteen lib-test
diagnostic reports, including shared DB roots: command refusal branch order;
explicit federation/remote-origin variant matches and semicolons; six concrete
BTreeSet defaults; two test aliases after statements; and one 102-line migration
fixture. The corrective source addresses these roots without
new lint allowances or removed assertions; the last fixture uses a scalar-query
future instead of a redundant async row wrapper. Migration edits are test-only:
registered SQL, schema132 and the registry remain unchanged. The correction has
had a staged-only read-only source review, not compiler/test qualification.
Prepared-management checkpoint work is a separate source-only slice; see the
internal Coordinator checkpoint boundary below.

Run **39363163071** captured exactd4336ee6f0cab3ceed52bf1ccf9b66ef967d49a4;
normal Rust was still running when its completed Clippy job **118433878610** was
inspected. The prior DB blockers cleared far enough to expose IDE diagnostics.
Owned Stop roots are source-corrected with let-else, retaining token cancellation
and the same typed GoalControlRevoked outcome. A shorter local fence binding
keeps the same constructor fence/subscriber/cancellation ownership while bringing
the literal below the reported length limit, without a new lint allowance.
Shared federation/send-chat/global-read/test diagnostics remain unwaived; this
is not a passing Clippy or executed-test claim.

The existing UI InputOrigin/EventPresentation generated contracts remain stale.
The Rust lane already runs ts-rs exports into `target/codegen-scratch`, but its
artifact retained only logs. A collection-only workflow correction adds those
scratch TS outputs to the existing artifact—no command, job, run, compiler step,
timeout, worker budget or generation behavior changes. The already-captured
d433 run cannot acquire this new collection retroactively. Future retrieval must
verify successful export and exact producer checkout, inspect the artifact's new
multi-root layout, and copy complete producer bytes verbatim while preserving
the hand-authored sse.ts barrel. No generated files have been hand-edited; until
that output is captured and committed the UI blocker remains real.

The subsequent Stop-ordering and identity corrections remain source-only until
an exact corrected-head run. No duplicate CI or local heavy execution was
performed. Production stays Disabled, emitter None, route503; actual133 is
absent and134 remains unregistered.

### Internal Coordinator checkpoint boundary

The parent captured run **39368026538** at exact
`e4d0d615061a0d927cc5ad3d1a3d875c7c375754`. Its completed Clippy job
**118449313750** reports the owned `Runtime::new` constructor at101/100 lines;
two temporary bindings are inlined in later source without changing channel or
catalog ownership or adding an allowance. The lib-test diagnostic tail names
shared federation fixtures, not additional owned goal-test diagnostics. The run's
Rust result and generated artifact qualification remain parent-owned. The terminal
run's focused selection passed **156/156** in152.811s with3391 skipped, including
the four prior owned failures and the mixed report/MCP regression. This establishes
a focused green floor only at exact`e4d`; it does not qualify later management
commits. The broad run reported5460 tests:5428 passed,32 failed and191 skipped;
those failures, Clippy and UI remain unwaived.

Artifact **12241064742** contains the successful exact`e4d` ts-rs producer output
under `target/codegen-scratch/ui/src/generated/`; `check.log` records compile214.3s
and codegen142.2s success followed by the stale-output guard. All185 artifact
exports were compared byte-for-byte with the repository. Eight generated files
were copied verbatim: six missing federation types and the stale
`EventPresentation.ts`/`InputOrigin.ts`. Handwritten `sse.ts` is the sole
repository-only TypeScript file and remains byte-identical. No local export or
build was run. These recovered bytes still require qualification with their
consumer UI; copying them is not a UI passing claim.

Internal `goal_manage` preparation now requires the parent Coordinator, its
canonical current assistant/tool-use, and a live generation-fenced materialized
Runtime turn. A pending intent carries no success output. Its checkpoint owner
acquires each ordinary target's product-message and transcript-acceptance gates
with try-only admission, then rechecks latest/settled target state transactionally.
Goal/history, canonical assistant/results, state, replay changes, and optional
terminal ownership commit together; exact retained results bypass target gates.
No ordinary owning-agent authority is widened to cross-owner management.

Source review corrected the reducer fixture to the actual `ToolCall::new` plus
`ToolInput::from_name_and_value` API. Fresh Close fences and history-only targets
are canonical management refusals through the two typed admission errors; source
authority, SQL/serialization and integrity failures are not reclassified. Added
source regressions cover free-gate Close/history targets in both Runtime and
terminal checkpoints, exact retained refusal replay, and unchanged target history.

New source regressions cover serialization, canonical prepared-event rejection,
real Coordinator materialization, gate refusal, rollback, historical retry, and
terminal checkpoint ownership. They have not been compiled or executed locally.
A later source-only provider journey exposes the internal declaration only when
the parent Coordinator has an explicitly installed management owner. The default
remains `None`. Its declaration uses the actual tagged `GoalCommandDisposition`
object contract, not string status names. The provider fixture materializes the
real source turn, emits a canonical tool call with signed Anthropic replay, and
drives the queued preparation event through checkpoint and following provider
request. Source tests cover Create and tagged state commands, replay rollback and
retry, missing/stale/ordinary authority, forged arguments/call identity, host
generation change, source retirement after preparation, and unavailable
capability advertisement. No generic tool executor applies management mutations.
The fixture is shared with service tests rather than introducing a second fake
source model. A bounded source review corrected helper/signature/schema errors;
the corrected follow-up found no further concrete source failures, not a passing
compiler or runtime qualification. Captured run39371603386 is at exact`9a3` and
does not contain this later journey work; its monitoring remains parent-owned.

Run **39371603386** at exact`9a3bf84748403b1607ea381337b48fe29bf45775`
terminated before focused execution: the materialized-turn fixture imported four
prepared/submitted payload types from `phoenix_workflow` instead of their actual
`phoenix_core::domain::sm_event` declarations. The shared extracted fixture now
uses those declarations. The owned Clippy panic-documentation failure on
`GoalPendingManagement::mutation` has an explicit `# Panics` section. Both owned
test barrier waits identified by `rust-test-timing` are bounded by30-second
outer liveness guards while retaining their Notify evidence. No ignore or lint
allowance was added. UI TypeScript, ESLint, Stylelint, Vitest and ast-grep passed
in that run; the UI/spec lane failed only on the two timing findings. Zero new
focused tests executed, so the focused156-test green floor remains exact`e4d`.

The provider journey's second-request assertion selects the exact user-role
`tool_result` by call ID and compares its content and non-error flag with the
persisted canonical result. It also checks ordered signed-thinking/tool-use
blocks and consistent Opus model identity. Tagged-state tests include a valid
blocked-to-underway recovery and schema-object representations for all four
states. These later corrections are source-only, not qualified results.

The parent verified actual PR902 source at
`83478ea102d5a5e346c32ca4862aeebba1242a27` with schema133
`opaque_federation_peer_notes`, and exact byte parity of its schema132 with the
accepted goal schema. That evidence supersedes earlier absence statements;
the narrow non-destructive133 source integration is a separate source-only cut.
Its registry entry and SQL are byte-identical to that retained remote source:
only the bounded opaque text `notes` column is added. No federation handler,
transport, tool, UI, or credential behavior is imported. The existing populated
connection preservation/UTF-8 byte-boundary test is copied from the same source.
A new fresh-schema regression covers default empty notes,32768-byte acceptance,
oversize/null/blob rejection, ledger133, idempotence, and absent134 registration.
The schema130 full-upgrade fixture stamps132/133 only to defer those unrelated
steps while inspecting130. It then removes those temporary stamps and executes
the real132/133 migrations before comparing the complete upgraded schema with a
fresh database; the132 stamp is not treated as proof of schema application.
Schema132 SQL remains unchanged. Formatting, diff checks and binary-safe remote
registry/SQL comparison passed; no migration SQL or tests were executed locally.
Schema134 remains unregistered through source review and hosted qualification.

The production service/emitter is absent, public tool registration and UI remain
unfinished, and watches/autocontinuation are not claimed. Schema132 is untouched;
134 remains unregistered and activation remains off.

### Earlier64322 compile diagnostic

Run **39348937685**, exact `64322db7480f1452d00357eaf20c209c16a54003`, is terminal
**FAILED**. Rust job118388364345/artifact12231779623 failed before test execution
in both normal and focused stages: two candidate-test helpers resolved the parent
one-parameter `Result` alias with two arguments (four diagnostics), and the IDE
Blocked-report fixture omitted the checked GoalReportBinding conversion. **Zero
new focused tests executed.** This does not supersede e19's actual109/5 result or
f6's then-current green96/96 focused floor; the later2ec127/127 result supersedes both as the current floor.

All identified owned compiler/lint roots are source-corrected after64322, including
explicit standard Result types, checked binding conversion and borrowed HTTP
error classification. Source audit also corrected the minimal schema fixture's
settled-admission setup and switched its Goal/NoGoal capture to the actual
`capture_goal_at_new_turn_tx` production helper. None of those later repairs or
the transactional control-recheck increment has exact-head execution evidence.
The parent subsequently commissioned exact2ec run39349884955, whose terminal127/127 focused evidence is recorded above. It does not cover later source. No duplicate run was dispatched.

E2E, geometry, task validation and worker-budget jobs passed separately. Clippy
and UI/specs failed; their remaining federation/shared paths are unchanged from
ed25 by source diff, not waived as green.134 stays unregistered pending actual133;
132 is unchanged. Private emitter stays None and the public control route stays
503. UserControl actor-cycle upgrade and installed runtime/tools/UI/watch/automatic
continuation remain commissioned and incomplete.

## Installed-path source, not execution-qualified

Parent-owned run39349884955 has captured exact2ec. The subsequent source increment
threads a typed default-disabled installation through Database clones and its real
WorkflowRepository projection. Only the explicit isolated test-support candidate
installer can enable it; schema presence is not activation. Actual new Runtime
acceptance captures control atomically with Goal/NoGoal, and all report transaction
owners recheck control before fresh mutation. Retained success/refusal stays
historical evidence after Stop/release. Missing schema is a storage failure.

Mandatory MessageStore forwarding now reaches actual model and tool dispatch,
independently of an emitter or Goal-vs-NoGoal. It checks the active materialized
turn/generation and retained control. Source now carries a shared per-owner
permit through final external polling: control commits and each future poll use
the same owner gate; Stop revokes exact permits irreversibly, and Release cannot
revive an old operation. Deterministic Goal/NoGoal barriers cover pre-first-poll,
inflight provider/tool cancellation, and SQLite writer contention. Revoked-tool
settlement composes cancellation and completion in one pure reducer result, so
a failed terminal write retains the existing retryable settlement rather than
losing the sole outcome at an intermediate state write. Actual provider and
tool settlement assert the durable Cancelled result; tool tests also inject a
real terminal checkpoint failure and retry. These new regressions have not
been executed. Production constructors stay Disabled, emitter None and control
route503; source inspection does not qualify production activation.

Coverage targets real acceptance, commit cuts, genuine claim/materialization,
nine report-owner shapes and runtime provider/tool effects. The newer dispatch
barriers and changed settlement cases remain execution-unqualified.
Read-only review caught/fixed the admission-vs-effect canonical-message predicate
and canonical-ID expectation before publishing. Parent2ec Clippy's two owned
fixture lints are source-fixed here; that is not test qualification. UserControl
requires a goal-command-specific receipt, never a Stop/release receipt masquerading
as goal creation. Later candidate source supplies the typed actor and cyclic
rebuild groundwork described below; authenticated command service and complete
upgrade qualification remain owed.134 is still unregistered pending actual133.
Runtime cancellation/linearization qualification, service, public tools/UI/watch
and automatic continuation remain unfinished.

## Stop ownership boundary and dormant authenticated Create

The permit orders **each synchronous parent-future poll** against a control commit
on clones of the same Database instance, not just the first poll. An added
Goal/NoGoal two-poll regression returns Pending before a deferred send, commits
Stop then Release, and asserts that the second inner poll/send never occurs.
It does not prove remote quiescence, undo effects already handed to a backend,
or establish cross-process/cross-Database ordering. Inflight teardown retains the
existing cancellation semantics, including asynchronous resource reclamation
(REQ-BED-005/005a), rather than a new strict remote transport contract.

Actual handoffs inspected: Anthropic/OpenAI HTTP streaming awaits reqwest send
and response consumption inside the guarded completion future; Codex WebSocket
completion uses the shared backend session/socket. Dropping that completion
future stops its local polling, not a provider request already transmitted.
`ToolRegistryExecutor::execute` awaits the selected tool run; MCP adapters pass
`ToolContext.cancel` through `McpClientManager::call_tool_cancellable_with_schema`
to its managed actor/transport path. The actor/backend can already own queued
work when the parent future drops; cancellation remains the existing token and
transport behavior, not remote rollback. Bash process ownership and reclaiming
remain with its existing process/handle registry. Unowned delegation through
`spawn_agents` is refused on the installed goal path. None of this authorizes
new goal turns or revives an old execution after Release.

Source after exact87 adds a default-disabled authenticated Create service and
its typed DB primitive. Owner authentication is required before host principal
derivation; the DTO cannot supply principal, actor or goal identity. Existing
product-message and transcript-acceptance gates select fresh admission versus
a retained busy refusal. A single owning transaction revalidates the live
ordinary owner before receipt access, retains exact refusal/replay evidence,
and commits the accepted request, canonical goal identity/history and shared
UserControl attribution together. Accepted objective replay reads canonical
identity; same-text replacement creates a fresh identity. Create neither inserts
chat nor clears Stop. Four authored, unexecuted service tests cover attribution,
held gates and historical retry, exact-input conflict, Stop preservation, actual
history-insert rollback and auth/default-off boundaries. This is not public
installation or State/AUQ/tools/UI/watch/automatic-continuation completion.
Parent-owned39358832109 captured exact87 and parent-owned39361606004 captured
exact387; both are terminal compile-red with zero focused execution. Subsequent
source corrections are unqualified. No replacement or duplicate run has been
dispatched by this writer.

The next shared `user_commands` source owner extends the same transaction and
host gate path to explicit State commands: Blocked recovery to Underway retains
the goal identity/objective; terminal revival and wrong target are retained
transition refusals. Accepted State references its own exact command receipt and
UserControl actor, not a fabricated Create receipt. User input structurally
excludes legacy Suspended. State and Create preserve Stop and do not materialize
a turn. Additional source-only regressions cover accepted historical State replay,
changed reason/command, terminal and wrong-target refusals, real State-insert
rollback, and a persisted structured-question wait that remains Underway and
retains a busy refusal even after becoming Idle. Parent-requested Create cases
also retry Busy after gate release at the same otherwise-eligible owner version,
refuse changed expected version/rejected objective, and exercise genuine
continuation followed by stale-prior-transcript and real Close fences before
retained replay. Nine service tests are authored, none executed locally.
Read-only review caught a nonexistent wrapped-close error pattern and a missing
rather than genuinely stale transcript fixture; both are source-corrected.
This remains default-disabled and unqualified, with public command authority,
owning-agent/Global tools, UI/watch and automatic-continuation integration owed.

## UserControl candidate source, not installed

REQ-GOAL-004.1 and ADR-183 record the no-chat, command-specific attribution contract;
application authentication and public command installation remain unfinished.

Typed UserControl actor metadata now carries UUID principal/request and explicit
Create/State command kind through actual core validation and DB insert/decode.
Legacy132 actor paths never query absent candidate columns. An unregistered SQL
candidate stages/restores the populated actor/history/binding cycle under deferred
FKs without revalidating historical message/admission locators. Its separate
accepted goal-command receipts bind exact actor owner/request/command/result and
history target; accepted payload reads canonical identity/history rather than a
second authoritative copy. Stop/release evidence is not a goal-command receipt.

The new actor is unique per accepted request and deferred-FK-bound into both
attribution slots of that command's exact history row; actual insertion reuses it
for direct requested/applied roles. An AFTER INSERT guard plus retained history
FKs rejects value/row-ID replacement. Candidate receipt INSERT-conflict guards
also prevent SQLite REPLACE from rewriting retry evidence, rewinding control or
recapturing an old binding while preserving normal one-step control UPSERT.
The candidate-only identity INSERT guard also refuses goal-ID or explicit-rowid
REPLACE, even with recursive triggers disabled and foreign keys deferred. Its
regression retains canonical accepted Create input, full history and the exact
receipt across failed replacement attempts. Registered132 remains unchanged.

Exact4e executed four UserControl tests: mid-rebuild rollback, receipt
immutability and positive typed Create attribution passed; populated132 retained
history/source-removal decoding failed at the cached projection boundary above.
The corrected decoder and expanded identity-REPLACE regression remain
execution-unqualified. This is isolated storage composition, not public
authentication or actual133→134 qualification. Accepted State execution,
complete legacy upgrade qualification and public authenticated command-service
installation remain owed. Legacy132 actor
REPLACE behavior outside the new receipt-backed history is captured as a TODO.
Read-only reviews corrected concrete API/SQLx-static-SQL/fixture issues before
publication; none is compilation or test qualification. Parent2ec CI does not
cover this later source. No migration registration or production activation.

## Historical foundation qualification

Existing CI39320605328 tested exact **`ed25a228e092b41d5c112e1c7585aab406944ca3`**. Rust job118293499801's focused nextest step **executed70 tests:70 passed,3397 skipped/filtered** (39.155s), with individual statuses/full assertions in artifact12221238348. This is actual bounded foundation qualification, not an authored-count inference or complete PR/product qualification. Individual passes include registered fresh initialization/replay, prior-schema upgrade/idempotence, real-migration supported hard deletion, admission/service/capture, actual settled continuation, and Steering ownership/replay/commit-cut regressions.

Earlier exactbe398 focused execution was60pass/5fail. Four fixture contradictions were repaired through actual state/summary/opt-in APIs and removal of invalid reservation teardown;913a3c599 added genuine settled-continuation positive coverage. Steering setup first lacked a Runtime owner, then exposed a real production defect: acceptance committed a Steering turn before validating its return step against an empty model. Source71e344215 now derives that step from the actual transactionally loaded owner before inserts and returns only after commit/cuts. Three generic regressions and the capture invariant passed at the qualified head. The isolated history-cursor arity regression at1c521 failed compilation, not tests;9787 corrected it. No production trigger, admission requirement or ownership rule was weakened.

The full Rust lane remains independently red:5213/5377 executed,5175 passed,38 failed,191 skipped;164 were not run after SIGTERM. Failure names outside the focused subset are not sufficient to assign cause/ownership, and normal output is tail-limited. UI wire parity and Effect Clippy failures are separate blockers. The earlier focused-first experiment timed out during cold compilation (exit124), not a product assertion; the qualified workflow runs the same bounded focused command after normal compilation with no normal-lane skip. No additional resources or retry framework were added.

Published source through `fd5a46849c31308d4f85658ba937334866e062e5` is **not covered by the green checkpoint above**. Ordinary, Anthropic/Responses replay, fork and terminal transaction owners share pending-round finalization; provider replay co-commits with either side owner. Persisted `ToolExecuting`/`CancellingTool` state and `CheckpointData` retain typed pending intent, and actual executor checkpoints install committed nonterminal state/results before publication. The existing terminal establishment carries its committed payload and probes exact retained rows, arguments, binding/history and provenance without predicting results or rerunning mutation; selected terminal state remains in the existing obligation until `TurnCommand` settlement. Interrupted uncommitted reports become canonical paired errors, while staged terminal obligations remain excluded from startup materialization. Durable trusted-tool projection shares the existing redaction policy without changing legacy `ToolResult` encoding.

The historical f6 source checkpoint recorded CI39331620726 at exact `f6a5ea5570a3d4a086ef4a427b9c4f7745c70e08` as pending; the terminal96/96 evidence recorded below supersedes that pending state. Its completed Clippy/UI logs exposed own exhaustive-match/manual-savepoint lint failures, corrected in the newer source with exhaustive matches and SQLx-owned nested transactions, alongside separate federation failures. None of those source corrections or the new runtime/combined-owner regressions has execution qualification yet. `Event::ToolComplete` remains finalized-only: there is no production pending-report emitter, public report tool or automatic runtime activation. Stop/UserControl fencing after the separately owned federation migration remains an activation gate; schema132 is unchanged. Remaining runtime emitter/context, tools, UI and watch delivery is still in scope. Earlier zero-execution/source-only statements below describe historical checkpoints, not qualification of this increment.

The three installer historical-registry fixture repairs at431296d7 and8860bf87 passed the actual `dev.py unit tests` stage at86ab (UI job118227195421,93.9s). Production helper targets/pins and actual-newer-schema rejection are unchanged. Inherited federation UI wire parity and large Effect Clippy failures remain separate blockers.

## Status

M1 source is authored as a dormant core/DB increment under the approved immutable-objective and source-registration amendments. Migration **132** registers the single `crates/phoenix-db/src/conversation_goals/schema.sql` source after coordinated federation migrations130/131 on exact source stack `f744df85865180597118c08b4f142b27fda57ee1`; non-destructive merge `b4f2aa52eee7b3ad560be2a2baa6c48675742758` preserves both histories. Normal fresh initialization replays the same migration, with no duplicate base-DDL application. ADR-182 records the bounded decision. Later source adds a default-disabled owner-authenticated control HTTP endpoint and private test-only emitter installation. Public goal tools, UI/SSE, automatic follow-through, durable runtime Stop integration and goal watch delivery remain uninstalled. Source registration is not a production migration or feature activation.

The original mutable-objective source at `4a71cba3976a2c3f1f02c1e1ef6931e27887907c` is preserved historical evidence, not the selected contract. `f6604cd89afdf93071d482ab0d38593c78e114d6` records the amended approval brief, not the completed feature. Subsequent source implements the immutable model. No compatibility bridge/backfill for the stale DDL is needed: it was never registered or applied.

Local validation is source-only: existing standalone rustfmt and Git whitespace checks. Earlier independent read-only reviews identified useful safeguards retained by the amendment. Initial hosted Rust job118165296336/run39284130828 at `d2c50bf8432aac3f8898d346e4ac36a0bed20127` failed lib-test compilation on seven dynamic-SQL `SqlSafeStr` E0277 errors; goal tests did not execute. Qualification-only fix `5e0edc288762ab298892c27a98a570f0a7079ec4` uses an exhaustive literal statement allowlist and bound values, without an SQL safety assertion bypass. Its normal hosted run39286379469 was queued at last inspection. That historical checkpoint did not qualify goals; the bounded passing execution is recorded above, with full-repository limitations retained. No local compiler/Cargo/test/SQLite/migration/browser/VM/install work or production execution occurred. Registered candidate awaits exact-head hosted checks and independent source review.

### M2 source application boundary

A dormant crate-private `goal_service::{get,apply}` is authored, with no public adapter or automatic follow-up caller. It uses nonwaiting acquisition of the existing product-message gate followed by transcript acceptance; an occupied gate returns Busy rather than queuing a Set/Replace that could execute later. The write transaction reuses ordinary aggregate lifecycle admission and the existing transactional ProductConversation chain/latest projection, then checks actual durable pending turns, steering, continuation admissions/reservations/intents, creation delivery and terminal obligations. It does not infer settlement from Idle alone. Exact live report commands bypass these wait gates but require their current Runtime turn/workflow and persisted binding. Exact accepted retries are historical reads, even while busy.

M2 adds **13 DB and 4 application-service tests authored**, not executed, to the 32 M1 tests. The new tests cover real migrated fixtures, refusal without partial actor/history writes, active Runtime and accepted Steering, queued input, AUQ/task-approval waiting, pending terminal settlement and creation delivery, reservation and continuation phases, changed latest transcript/opening intent, live-report versus owning-agent Set, exact retry, gate refusal and persisted Get. They do not qualify trusted public tool attribution, Stop enforcement or runtime continuation.

Exact M1 checkpoint `c7d9794f5c8beb06cd634e8de1a3458b268be3a3` normal run39287295698 failed before goal tests compiled: inherited federation `mod presentation` was missing its source file; inherited federation Clippy, UI generated-type and test-timing failures were separately classified. The goal-owned ADR182 metadata-bullet error was repaired in `95cce0a56`. Native run39287295187 reported 346 pass/1 guardian-frame-timeout failure on unrelated controller paths, not goal-test evidence. Parent coordinates actual federation owner correction/import. **No goal test pass is established.** Migration132 SQL and registration remain unchanged by M2.

### Admission capture source increment

`WorkflowRepository::accept_authoritative_turn_at_cut` captures Goal/NoGoal in the transaction that creates each ordinary Runtime turn, before both existing commit crash cuts. It derives owner and latest status from the co-created durable turn and existing immutable history. Non-Underway states capture NoGoal at the observed version, not permission to resume a Blocked or terminal goal. Accepted Steering alone does not capture execution authority; accepted-turn replay returns before capture. `conversation_goal_execution_binding` reads one exact workflow/turn's stored observation and never backfills a missing binding. The old supplied-binding primitive is test-only. No provider request context, public tool, continuation or Stop behavior is activated by this increment.

Five admission regressions are authored in `workflow/direct_turn/tests/goal_capture.rs`: no-goal exact replay, Underway fence plus wrong-workflow refusal, Blocked nonactivation, before/after-commit atomicity, and steering exclusion. Combined authored count is **54**, not a passing-test claim. M2 normal CI39290326736/Rust118189160877 remains blocked by inherited federation compile errors (including missing presentation, obsolete federation test APIs/fields and dynamic-SQL test string); no goal pass is established. M1's132 SQL/registry remains unchanged.

### Pending report checkpoint preparation

Independent exact-HEAD source review at `78591f52a` found two **goal-owned** unconditional core compile defects: the new round wrapper assumed a nonexistent `ToolResult.tool_name`, an integer rather than optional duration, and a four-argument success constructor; the new report binding named a nonexistent `GoalError::StaleFence`. Source repair removes the invented name accessor, preserves optional duration, uses the actual two-String constructor, and reuses `GoalError::Conflict`. This is source-reviewed correction, not hosted passing-test evidence; these new blockers must not be attributed to federation or conflated with earlier4a diagnostics. A subsequent source audit found one additional goal-owned test compile defect: the outer-transaction fixture named nonexistent `GoalDbError::ActorMismatch`. It now expects the actual `Integrity("source message missing")` refusal and rolls back that failed transaction before staging the valid complete round in a fresh transaction.

Specific `GoalPendingReport`, checked `GoalReportBinding`, and `ToolRoundResult::{Finalized,PendingGoalReport}` data types are authored, but are not yet threaded into the runtime state machine or checkpoint DTO. A report stores one binding/fence and call identity; owner, version and owning execution are derived for the mutation instead of stored twice. NoGoal, mismatched owner and zero-version report bindings are refused. Pending outcomes can be Blocked, Complete or Exhausted, not implicit Stop/resume. The round wrapper has no success/output methods; ordinary finalized `ToolResult` encoding is unchanged, and an ambiguous pending-plus-finalized payload is rejected. Seven new pure tests bring the authored inventory to **65; zero proved passing**. Report action identity is deterministically derived from the admitted owner/goal/version/workflow/turn, actual call IDs and intended status/reason payload using the existing exact-payload fingerprint. It is not a caller-supplied or redundantly stored identifier; altered intent cannot reuse the same computed action identity. The finalizer still must verify the genuine immutable assistant arguments before reusing any committed rejection or success. These types do not authenticate source provenance, finalize a tool result or qualify recovery by themselves.

The goal reducer/write body is factored as private `apply_goal_tx`, leaving transaction establishment/commit in the existing application facade. This enables the same checks to join the actual tool-round transaction without a second goal transaction or weakened actor validation. Two additional authored DB tests insert the genuine assistant and paired result in the outer transaction, then assert rollback or commit/exact-retry behavior. They are unexecuted (58 authored at the transaction-seam checkpoint; zero proved passing). No pending tool-result variant, savepoint rejection finalizer, provider replay integration or recovery guarantee is established by this refactor.

Goal mutation, actor and immutable binding vocabulary has lossless serde encoding for the existing earned runtime-state/checkpoint aggregate. Checked text/counter/time/execution decoders invoke their constructors, rather than permitting blank identifiers or invalid IDs through derived deserialization. Required command fields have no defaults or skip-serialization paths. Two pure regressions cover exact command/actor/fence roundtrip and invalid/missing-field refusal (56 authored at the encoding checkpoint; zero proved passing). This encoding is data, not source authentication or execution authority; a pending ToolResult variant and full-round finalizer are not installed by this preparation.

The exact federation source-repair descendant `866e5e56dd4d6a2276b2a741bd2459629af3dd7d` was non-destructively merged after parent integration approval. Its seven-file diff from the coordinated f744 base adds the genuine missing presentation module and owner-authored source/client corrections, without migrations or ADR changes. This is not federation qualification or a new schema reservation. A separate source-reviewed capture-test fix names the goal identity on its Blocked disposition request. Normal run39291963325 at exact `4a87f501256f9fc9e58755e89a9f834a31caa7e4` is terminal failure: the genuine presentation module has unused functions under `-D warnings`, DB tests retain 15 federation compile errors, and inherited Clippy/UI-generated-type/Python-client failures persist. No goal test executed. The compact source/job blocker packet is preserved on draft PR959 as [comment6273105996](https://github.com/scottopell/phoenix-ide/pull/959#issuecomment-6273105996); actual owner correction is required rather than suppression or fabricated APIs.

### Actual-signature audit and corrected hosted target

Exact owner repair `5d8fda7f37a783cddd444c639fce2a8457998bc2` was imported non-destructively as `c7e81412f099ace91a3981d179fc3b0b1d41e34e`, with no132 or133 dependency/change. Existing dispatched CI39297246365 at exactc7 failed. Rust118212999376 and Clippy118212999377 confirmed three remaining goals-owned `ProductConversationId::from_db` fixture calls; isolated correction `c42b80e45f89b4920d7550819ccb9a8e384068c3` uses the actual checked parser. Earlier source review had left these calls behind. The bounded actual-symbol audit covers new DTOs/tests, goal DB/service/capture helpers, Message/ToolResult shapes and all existing full-round transaction owners. Source inspection and rustfmt are not compiler/test qualification. Existing CI39298092785 targets exactc42; no goal execution/pass is established yet.

The c7 Rust job separately reports four inherited IDE test compile errors in `api/federation_two_instance/subscriptions.rs`: inaccessible RESPONSE_BYTES, nonexistent PeerBaseUrl.as_str, unsupported TranscriptConversationId FromStr, and obsolete revoke_federation_enrollment argument. The file has identical Git blob at exact866, exact5d8 and c7. The same c7 run passed E2E118212999567, assistant geometry118212999333 and task validation118212999165; those successes do not qualify goals.

UI/specs118212999409 failed inherited generated federation type parity/test-timing checks and Python prepared-upgrade fixtures. `CurrentSchemaPreparedUpgradeTests.setUp` models a historical qualified128 candidate by removing only129 from the evolving registry: inherited130/131 remain, and goals132 additionally exposes that coupling. Production helper correctly fails closed for newer-than-qualified targets. Fixture isolation requires deployment-owner coordination; no production target/allowlist/hash expansion is implemented. Hosted failure-tail output is not evidence that every distinct cause of aggregate76fail/203error has been classified. Exact source/run/job packet is retained in PR959 [comment6273105996](https://github.com/scottopell/phoenix-ide/pull/959#issuecomment-6273105996).

The actual production replay transaction owner is `Database::persist_tool_round_state_and_replay` in `provider_replay.rs`; the similarly named DB `persist_tool_round_state_and_provider_replay` facade is test-only. The runtime adapter invokes the production owner. Other existing owners are `persist_tool_round`, `persist_tool_round_and_state`, `persist_tool_round_with_terminal_obligation`, and `persist_fork_proposal_with_tool_round`. None consumes pending reports yet. The retained shared finalizer is an uncommitted private draft, not a usable report/recovery integration; exact checkpoint-assistant/canonical-result checks do not replace atomic runtime-state and replay finalization.

## Selected model

- Ordinary stable ProductConversation owns each immutable goal identity and objective; no WorkScope is required. Objective is stored once on the immutable identity row, never repeated as writable history data.
- Goal-specific immutable status-history rows are the only current/history authority. Current identity/status is the indexed latest owner version. Creating a new identity atomically supersedes that current identity, preserving prior identities and factual outcomes rather than editing text, clearing first or fabricating success.
- There is one monotonic owner/status version across identities. Every actor uses observed-version CAS; exact action retries compare the original full payload/actors/binding and return the original history result even after terminal settlement or supersession.
- Underway and Blocked are nonterminal; explicit Blocked → Underway preserves identity and objective. Complete and Exhausted are distinct terminal outcomes: exhaustion is unfinished, not success. Neither terminal identity can be reopened or receive another status change. A deliberate new goal has a different identity.
- There is no objective-edit, objective-revision, clear API, lifetime/shared-budget schema or wait framework. Existing Bash same-handle waits are unchanged.
- Concrete requester and applier occupy normalized actor rows. Agent validation checks owner/kind/workflow/transcript membership and an actual typed assistant ToolUse. This is consistency, not authentication or proof that the tool belongs to that exact turn; exact invocation assignment must come from trusted runtime context.
- An immutable binding stores either Goal or NoGoal at an observed owner version, indexed by durable turn. Captured goal identity plus version fences stale reports after status changes or supersession, including identical replacement text.
- Immutable source locators may outlive deleted source rows. Goal data cascades with its ordinary owner; standalone history/actor/binding deletion is guarded while that owner exists. Full-schema supported hard-delete tests are authored for superseded/terminal goals and NoGoal-only bindings, not executed.

## Explicit runtime integration limits

Storage is **not** Stop protection or start permission. A replacement's Underway marker does not release user Stop/security. Dormant Suspended remains solely a stored fencing representation with no status-release operation, not a commissioned resume policy. M3 must identify and enforce the actual durable runtime Stop fence across replacement before activation.

Ordinary Runtime admission co-creates an immutable Goal/NoGoal observation; exact replay does not recapture, and Steering is excluded. No provider request consumes that context yet. Trusted report adapters must use this exact admitted execution, never look up a replacement goal when a queued report dispatches or retrofit a goal onto an older unbound turn.

A NoGoal observation cannot be silently converted into a goal-bearing binding. Approved composition keeps pending structured questions and task approvals orthogonal to goal status: waiting leaves the goal Underway and the harness must respect the existing execution suspension without a competing follow-up. For one issue, the agent either asks and awaits, or reports an explicit Blocked reason/action needed and finishes—not both. Environmental Blocked reasons contain no secrets. Blocked → Underway preserves the same identity when authorized; no automatic answer-to-goal-state coupling is installed here.

Blocked attention uses only the existing subscribed Global watch path; a notification grants no blanket answer/approval authority. Global action still requires independently verified standing user scope. Actual AUQ/task-approval dedupe, recovery and watch/runtime integration remain parent-owned outside M1. Persisted goal context on the next permitted request must not supply synthetic-chat start permission. The locked later mutation contract is a settled-turn boundary, not raw Idle: no active durable turn, backlog, queued admission, pending question or approval; busy operations reject without a synthetic queue. The dormant application service implements this admission contract separately from the storage migration; no public goal mutation caller is installed.

## Requirement and authored coverage map

All rows mean **authored; unqualified**, not shipped or verified behavior. The original immutable candidate authored 9 pure domain + 21 DB tests (30 total). The registered increment authors 9 + 23 (32 total), adding a genuine prior-schema upgrade case and a goal-bearing admission → successful report → supported hard-delete case. No actual goal pass is evidenced yet: the initial hosted Rust job failed compilation before execution. Full-substrate fixtures now use normal registration; only the isolated constraint fixture explicitly installs goal SQL.

Draft PR #959 runs existing normal hosted CI, without workflow edits, repeat dispatch or local compiler fallback. It includes inherited federation source, not newly authored goals functionality. Review the goals-only diff against `f744df85865180597118c08b4f142b27fda57ee1`; the overall PR base remains main `69b5d1b7018c62fb8c20fc9080e75b77131bc12b`. Attribute inherited failures separately rather than claiming the whole stack qualified from goal tests.

| Requirement | Source anchors | Authored regression anchors |
|---|---|---|
| REQ-GOAL-001 immutable ordinary ownership/current identity | `conversation_goal_ids`, `conversation_goal_owner_is_ordinary`, `Database::conversation_goal` | `wrong_owner_role_and_fabricated_user_channel_roll_back_every_row`, `immutable_objective_is_not_overwritten_by_create_reuse_or_raw_updates`, `superseded_goal_identity_cannot_be_reused_or_bypass_owner_version` |
| REQ-GOAL-002 all-writer owner CAS | `GoalMutation::advance`, `Database::apply_conversation_goal`, owner/version primary key | `all_actor_writes_compete_on_observed_version`, `objective_cannot_change_under_current_identity_and_supersession_needs_observed_version` |
| REQ-GOAL-003 atomic history/retry | action uniqueness; transaction-scoped identity and actor inserts | `normalized_history_roundtrips_all_actors_and_exact_retries`, `failed_history_insert_rolls_back_both_actor_rows`, `agent_locator_must_name_a_real_assistant_tool_call_for_both_actor_roles` |
| REQ-GOAL-004 actor distinction | `GoalActor`, `insert_actor`, `conversation_goal_actor_source` | all-actor round trip, wrong-role/channel, invented-call and user-message-as-agent cases |
| REQ-GOAL-004.1 no-chat controls | typed UserControl insert/decode and unregistered command-receipt/cyclic-rebuild candidate; owner command service not installed | authored storage/upgrade/Create attribution cases, unexecuted; authenticated service/exact State application remain owed |
| REQ-GOAL-005 execution/report fence | `GoalExecutionBinding`, `bind_goal_at_admission_tx`, `conversation_goal_report_fence` | `admitted_binding_is_immutable_and_late_report_cannot_rebind_to_replacement`, `raw_report_row_cannot_reference_another_owner_or_wrong_version`, `no_goal_admission_cannot_acquire_a_later_goal`, `replacement_changes_identity_and_fences_admitted_report_even_with_same_text` |
| REQ-GOAL-006 immutable identity/status terminality | `GoalChange`, `GoalDisposition`, `GoalMutation::advance`, `conversation_goal_history_transition` | `complete_and_exhausted_are_distinct_terminal_outcomes_and_cannot_reopen`, `blocked_underway_roundtrip_keeps_objective_and_fences_previous_execution`, `terminal_success_and_exhaustion_roundtrip_cannot_reopen_but_allow_new_identity`, `suspended_representation_rejects_fresh_report_or_status_release_without_claiming_stop_enforcement` |
| REQ-GOAL-007 scoped retention/deletion | owner cascades and owner-existence no-delete guards; evidence locators | `real_migrations_user_goal_supersession_and_supported_hard_delete_do_not_pin_aggregate`, `real_migrations_terminal_and_superseded_goals_do_not_pin_supported_hard_delete`, `real_migrations_no_goal_binding_does_not_pin_supported_hard_delete`, `history_and_binding_rows_cannot_be_rewritten_or_deleted`, `real_migrations_goal_binding_successful_report_and_hard_delete_remove_all_goal_rows` |
| REQ-GOAL-008 relational/time/upgrade | migration132/`schema.sql`, checked `_us` timestamp | `timestamp_constraints_reject_text_and_negative_integers`, `registered_schema_exists_after_fresh_normal_initialization_and_replay_is_noop`, `prior_schema_upgrade_adds_empty_goal_storage_preserves_source_rows_and_is_idempotent` |
| REQ-GOAL-009 settled application admission | `goal_service::apply`, `require_settled_tx`, `require_live_report_tx`, existing message gates and transactional aggregate projection | `conversation_goals/tests/admission.rs`; `goal_service/tests.rs` (17 authored; unexecuted) |
| REQ-GOAL-010 genuine-round report finalization | pending report/round vocabulary and private `apply_goal_tx` preparation only; runtime state, shared savepoint finalizer, finalized-result publication and provider replay integration remain uninstalled | pending/finalized encoding and outer-transaction rollback/commit tests authored; full crash/recovery/Stop cases still owed |

Core tests live in `crates/phoenix-core/src/domain/conversation_goal/tests.rs`; DB tests live in `crates/phoenix-db/src/conversation_goals/tests.rs`. The isolated fixture tests storage constraints; separately authored tests open the full migration substrate and invoke supported delete. The restart case exercises persisted binding/history retry only, not startup runtime recovery or context-continuation qualification. Earlier edit/clear regression cases were rewritten as direct supersession, immutable-objective, terminality and owner-version cases rather than silently discarded.

## Handoff and remaining gates

Task98021 carries the approved amendment and status. Migration132 and ADR182 reservation gates are resolved; exact registered-source review and hosted compile/test/check qualification remain. Settled-turn application admission is authored but remains unqualified and has no public caller. Atomic new-turn observation capture is authored separately. Actual trusted adapters, a stable-owner durable Stop fence and provider request preparation remain integration work under the locked design; follow-through is not activated. Keep132 immutable after qualification/import; any concrete Stop DDL requires later coordinated numbering after federation133. No extra implementation writer, generic authorization/event/wake framework, shared-main mutation, deployment, release or production migration is introduced by this increment.

ADR-182: immutable objectives remove the need for objective-revision fencing and edit/reopen policy. New identity plus the existing monotonic owner version fences replacement races; objective-once storage makes mutation structurally unavailable. Goal-specific history preserves terminal outcomes and exact actor retry evidence. Owner coordination released132 and182 before assignment; reserved federation130/131 and ADR180/181 are preserved.

## Private report emitter and runtime journey qualification

The source includes a parent-only `GoalReportPrepared` event and a private
runtime emitter whose production constructor remains `None`. The admitted
binding owns both the advertised `goal_report` definition and preparation of a
typed pending report. Only tests install this binding; this is not production
activation or authorization for another turn.

Six added `runtime::executor::goal_round_tests` journey functions process actual
provider outcomes and internal events through `process_event`, rather than
installing a synthetic pending checkpoint. They cover paired report rows, goal
mutation and committed state, public replay identity, the next Anthropic-rendered
request, rollback followed by manual exact-event retry, Stop before prepared-report
delivery, and strict report inputs. A seventh test checks only the parent-emitter
helper guard, not a child runtime journey. None proves automatic restart recovery.
A mixed report/proposal response remains all paired errors with no goal history;
a separate sole proposal reaches `AwaitingTaskApproval`. Database pending-fork
parity remains defensive coverage, not a runtime fork journey.

Exact head `f5549ac9a829e04728b98d6524df3c61d236c953` failed hosted run
`39338770106` before goal tests executed (Rust artifact `12228471018`). The
compile blockers were a shared replay validator import, replay helper visibility,
and the prepared-report cancellation signature; independent parent source review
also caught nonexistent binding metadata in the emitter description. Isolated
source corrections remain unqualified until another exact-head hosted execution.
The intervening runtime/provider/fork/terminal changes are also unqualified. The prior 96/96
focused pass belongs only to `f6a5ea5570a3d4a086ef4a427b9c4f7745c70e08`, run
`39331620726`, artifact `12226138977`; that run's aggregate result failed.
No local compilation, tests, migration, deployment, or activation was performed
for this source-only continuation. Migration132 is unchanged. UserControl and
stable-Stop have an explicit reservation134, with registration gated on actual
federation133 integration. Unregistered control SQL, a real trusted service and
an explicitly disabled HTTP adapter are authored separately; none installs the
control schema or activates the emitter. Actor-cycle upgrade, runtime admission,
continuation, tools/UI/watch and end-to-end feature delivery remain outstanding.

### First executed private-runtime qualification (partial, not green)

Exact head `e19c24639d915fb427b5ad509297fc37c62809b4` completed hosted run
`39343463637` with a failed aggregate result. Rust artifact `12231755781` contains
a focused result of **114 executed, 109 passed, 5 failed** in112.221s. Actual
private-emitter processing/next Anthropic rendering, checkpoint rollback/manual
retry, mixed report/proposal all-error refusal, separate sole proposal awaiting
approval, default-off/forged-input rejection and parent guard tests passed on
that head. The Stop journey failed; this is not complete journey qualification,
public admission installation or a full-feature pass. Later control schema,
service/API and source corrections are not included in that e19 evidence.

The five owned failures were traced to: cancelled report results reaching the
ordinary-success refusal as non-error cancellation content; a trusted-result
fixture with the wrong serde enum shape; a reducer fixture treating generic
Cancelled as Error; a cancelled-terminal fixture incorrectly expecting Retired
rather than known-not-committed generation refusal; and an illegal settled-only
state mutation used to set up a live-turn refusal. Source repairs keep the
pending-report success guard, generation/probe and settled-admission policies
intact. They remain unexecuted until a subsequent exact-head hosted run.

The last wholly green focused floor remains f6's96/96; e19's109/5 is a separate,
failed partial result, not an extension of that floor. Full normal checks also
failed; no blanket base-failure exemption or production approval follows.

Read-only source review of the five-root repair batch found no concrete new
regression; it is not execution qualification. Before the next single hosted
run, two newer control-candidate defects were corrected: a fixture's nonexistent
`InstanceId::from_uuid` call now uses the actual `new` constructor, and the
unregistered latch owner key is explicitly NOT NULL so SQLite cannot bypass its
owner/receipt foreign keys with NULL. An authored negative schema test asserts
SQLx's NotNullViolation kind. Candidate controls and all source repairs remain
unqualified until exact-head hosted execution. No registry/132 edit, runtime
installation, production activation or local test/migration was performed.

Corrected source64322 is under the sole new hosted run39348937685; no result is
inferred while it is running. Separately authored transaction helper
`require_current_control_tx` checks an execution's exact retained control version
and request against the current open latch and live ordinary Runtime owner,
without recapture or release. Two unexecuted regressions cover the actual DB
request operation, Stop→release stale-binding refusal, fresh execution admission,
cross-owner/workflow refusal, close gates and unclassified missing-schema errors;
the atomic Goal/NoGoal capture test also exercises the helper. This later source
is not in64322's CI and is not installed in report/model/continuation owners.

The completed Clippy job118388364386 from64322/run39348937685 found an owned
test compile blocker in the new Blocked-report setup: loaded execution binding
was passed without the existing checked conversion to GoalReportBinding. It
also found owned needless-by-value control HTTP error classification. Source
correction05acd5b72 fixes both, without assertion/guard changes; it is outside
the running64322 evidence and has not been re-dispatched. Twelve additional lib
lint diagnostics on federation/shared paths remain unwaived. No new focused
passing count is inferred before the Rust job/artifacts complete. The one new
run allowance has been used; later source needs exact-head hosted qualification.
