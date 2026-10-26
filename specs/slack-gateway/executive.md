# Slack gateway — Executive summary

## Requirements summary

One configured owner can flag a Slack discussion for a private briefing or
submit an explicit local agent request using a fire-prefixed message or fire on
their own message. Work remains an ordinary Phoenix conversation with native
permissions. Replies are intentional, source-bound and deduplicated. Discovery
covers four calendar dates and establishes a non-executing initial baseline.
Clarifications link to the ordinary Phoenix conversation for answers. That
handoff ends the automatic Slack response; native discussion stays private.

## Technical summary

The in-process gateway uses shared conversation services and the existing MCP
manager. It owns source discovery, association, acknowledgments and external
return evidence; it does not own another agent runtime or permission mode.
The lifecycle Allium model owns acceptance, native correlation and returns.
The companion discovery model owns call allowance, service opportunities,
pagination and initialization. Private notification providers are isolated in
an owner-configured local executable using a versioned stdin/stdout protocol.

## Status summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-SG-001 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-002 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-003 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-004 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-005 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-006 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-007 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-008 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-009 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-010 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-011 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-012 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-013 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-014 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-015 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-016 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |
| REQ-SG-017 | Partial | Implemented; operating-condition and production qualification remain bounded by evidence below. |

## Evidence and limits

The prototype completed private prefix, own-reaction, other-author briefing,
thread reuse, intentional publication, generic
private notification and retained-state deduplication checks. Source IDs,
workspace identities, recipient details, helper commands and provider-specific
operational records remain outside the public repository. Private prototype
commits are not ancestors of the publication branch.

The scheduling baseline passed 20 focused application and 50 database/replay/
watch tests, Allium structural checks, Clippy, formatting and spec checks. Its
adversarial author review found and fixed inactive acceptance being mistaken
for recorded evidence and acknowledgment throttling losing its backoff status.
These author reviews are not independent sign-off or a formal liveness proof.

The public candidate adds migration 132 after upstream's migration 131, replaces
the personal notifier with a provider-neutral command, and adds bounded
newest-prefix refresh. Focused gateway checks, the controller snapshot fixture,
Clippy, formatting, UI type checking/lint/tests, model checks and API end-to-end
checks have passed. The broad native run passed 5,319 tests and failed 14.
All nine historical migration failures passed after isolating their fixtures;
three shell runtime checks passed with the required native launcher fixture,
and the browser lifecycle check passed with a fresh temporary profile. The local
terminal-exit timeout remains a host qualification limit. The merged source's
24 gateway tests, Clippy, formatting and structural/timing checks pass.
Historical developer-tool registry fixtures have focused regression coverage;
the clean-environment full local run timed out, so hosted CI remains a gate.

The first public Codex review identified five source-verification, return,
question-recovery and configuration boundaries. Focused fixes use exact source
bodies, typed pre-dispatch evidence, native execution-receipt recovery, verified
authors for ambiguous echo matching and an atomic account-switch guard. The
expanded 233-test MCP/gateway run passed 232 cases; its remaining schema-error
assertion was updated to the new typed outcome and passed in isolation. Clippy,
formatting, Allium and structural/timing checks pass. The next public review and
full CI head remain qualification gates.

The second public review identified explicit notifier rejection, recovery replay
accounting, unavailable search hits and independent thread-root verification.
Those fixes passed the expanded 238-case MCP/gateway suite. A final two-case
one-shot test also confirms connection recovery settles without replaying a
read or write. The eight API-guidance checks follow the shared native question
service, and Clippy, formatting, Allium and structural/timing checks pass.
Latest-head hosted CI and automatic review remain gates.

The third public review added fixed-connection actor/workspace verification,
retryable notifier launch refusal, expired backoff cleanup, connector-metadata
separation, complete consumed-batch fallback coverage and permanent thread
restriction handling. All 244 affected MCP/gateway cases pass on the revised
source, with Clippy, formatting, Allium and structural/timing checks passing.
The mandatory workspace proof changes the backlog fixture to 16 calls over two
passes, with two context reads; the earlier 12-call result remains a historical
measurement before that verification. Latest-head CI and review remain gates.

The fourth review closes private-question publication through a lagging native
owner, preserves literal reaction-shaped instruction lines, shares settings
inspection's allowance/backoff with automatic service, guards unfinished
initialization against timezone changes and retries positively rejected
rate-limited sends. Exact source reads isolate the source timestamp; reaction
metadata requires independent evidence. All 248 affected MCP/gateway tests pass,
including shared inspection and private-input regressions. Clippy, formatting,
Allium and structural/timing checks pass. Hosted Rust CI on the preceding head
stopped at a Linux browser CLI teardown failure (`missing process group`);
latest-head hosted CI and automatic review still require qualification.

The exact server candidate `9648aa15576e056d878e73fae79af411a472921b`
passed online-backup and stopped-snapshot rehearsals from the installed complete
127 ledger to its exact complete 130 registry. Integrity, foreign keys,
conversation/message/product identities and message contents were preserved.
An explicitly approved manual offline cutover installed that candidate while
preserving installed configuration except the version value. This operator
pilot does not expand any automated deployment controller's qualified intervals,
provide automatic database rollback or qualify a generic 127-to-130 route.
That pilot's migration 130 is the pre-publication Slack allocation. Upstream
subsequently assigned 130 and 131 to continuation/capacity data; this branch
allocates Slack at 132 while preserving the upstream migrations and Slack SQL.
The pilot ledger is not the public candidate's ledger. No direct upgrade from
that pilot to the merged candidate is qualified; it requires separate offline
reconciliation and validation before any later production deployment.

Production initialization completed without executing historical triggers. Two
sequential explicit private-thread requests each produced exactly one native
input and confirmed reply, reused one conversation and survived a managed
restart without duplicates. The owner confirmed the notification alert and
reacted to the test notice. That gesture produced one native input in a separate
briefing conversation, assistant output and one exactly correlated confirmed
private return; the source thread received no reply. The bounded production
acceptance sequence passed. Only production discovery is enabled; the former dev watcher
is paused. Private operational evidence is retained outside the repository.

The native client recognizes Slack origins and source links, with version-2
transcript caches and a bounded version-1 migration. Local wire-model type
checking/round-trip execution and Swift parsing passed. Full native-client
build/test execution passed in hosted simulator CI. This host lacks the iOS
platform SDK and simulator runtime. The pinned local server predates the
publication branch's upstream integration; hosted checks qualify that integration
separately from the bounded operator pilot.

## Activation checklist

1. Pass focused scheduling, notification-protocol and native integration checks;
   validate Allium, formatting, Clippy and UI contracts.
2. Reconcile residual regression failures with the current candidate rather than
   treating historical failures as current evidence. The reproduced controller
   fixture failure is resolved by sealing its offline snapshot before the
   unchanged read-only controller checks it; unqualified targets still reject.
3. Establish the actual installed complete ledger, qualify the exact transition
   on disposable SQLite copies, and preserve the protected deployment contract.
4. Deploy the exact reviewed local candidate with discovery disabled and verify
   runtime identity and installed configuration; only one instance may watch.
5. Run a separately approved bounded private acceptance sequence, measure
   discovery-to-delivery stages and inspect exact deduplication evidence.
6. After local acceptance, publish only sanitized commits, open the PR, and
   resolve automated review feedback on each pushed head until review-clean.

Live automatic continuation and longer soak coverage remain unqualified until
exercised. They are follow-up confidence work rather than an invented unlimited
SLO for a bounded personal pilot. External shared-channel send limitations remain.
The clarification handoff replaces Slack answer inference with deep links into
Phoenix. Unsubmitted persisted question associations are rejected, native
answers retain their ordinary UI contract, and confirmed handoffs close the
original automatic response duty without inheriting private discussion.
After upstream reconciliation, all 387 MCP/gateway, migration and creation
delivery-replay tests pass, including the handoff, pending-question admission,
exact status-link and frozen-link retry regressions. All 235 deployment tests
pass with byte-exact historical registry fixtures and refusal of the newer
candidate by the protected target-131 route. Clippy, formatting, TypeScript,
Allium and structural/timing checks pass. The preceding public head
passed every required hosted check; the revised head still needs automatic
review and CI. The approved local pilot remains on its separately pinned build.

See [setup](../../docs/slack-gateway-setup.md),
[measurements](../../docs/slack-gateway-performance.md), and the two Allium files.
