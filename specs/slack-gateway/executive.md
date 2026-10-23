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

The public candidate updates against current upstream, adds one migration after
its existing registry, replaces the personal notification implementation with a
provider-neutral command, and adds bounded newest-prefix refresh. Qualification
of these changes and the exact installed production transition is in progress.
A new feature migration does not implicitly qualify deployment or rollback.

## Activation checklist

1. Pass focused scheduling, notification-protocol and native integration checks;
   validate Allium, formatting, Clippy and UI contracts.
2. Reconcile residual regression failures with the current candidate rather than
   treating historical failures as current evidence. The read-only controller
   integrity-check failure needs a reproduced disposition.
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
No live Slack write, production activation or public review is claimed by this
status document before it actually completes.

See [setup](../../docs/slack-gateway-setup.md),
[measurements](../../docs/slack-gateway-performance.md), and the two Allium files.
