# Slack gateway — Executive summary

## Requirements summary

One configured owner can flag a Slack discussion for a private briefing or
submit an explicit local agent request using a fire-prefixed message or fire on
their own message. Work remains an ordinary Phoenix conversation with native
permissions. Replies are intentional, source-bound and deduplicated. Discovery
covers four calendar dates and establishes a non-executing initial baseline.

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
thread reuse, exact native question/answer, intentional publication, generic
private notification and retained-state deduplication checks. Source IDs,
workspace identities, recipient details, helper commands and provider-specific
operational records remain outside the public repository. Private prototype
commits are not ancestors of the publication branch.

The scheduling baseline passed 20 focused application and 50 database/replay/
watch tests, Allium structural checks, Clippy, formatting and spec checks. Its
adversarial author review found and fixed inactive acceptance being mistaken
for recorded evidence and acknowledgment throttling losing its backoff status.
These author reviews are not independent sign-off or a formal liveness proof.

The public candidate adds migration 130 after upstream's migration 129, replaces
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

The exact server candidate `9648aa15576e056d878e73fae79af411a472921b`
passed online-backup and stopped-snapshot rehearsals from the installed complete
127 ledger to its exact complete 130 registry. Integrity, foreign keys,
conversation/message/product identities and message contents were preserved.
An explicitly approved manual offline cutover installed that candidate while
preserving installed configuration except the version value. This operator
pilot does not expand any automated deployment controller's qualified intervals,
provide automatic database rollback or qualify a generic 127-to-130 route.

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
Public PR review and remaining full-suite CI are in progress. The separately
approved bounded local acceptance sequence and native-client simulator CI passed.

See [setup](../../docs/slack-gateway-setup.md),
[measurements](../../docs/slack-gateway-performance.md), and the two Allium files.
