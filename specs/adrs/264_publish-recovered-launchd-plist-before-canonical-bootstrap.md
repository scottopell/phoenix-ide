# ADR-264: Publish recovered launchd plist before canonical bootstrap

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** launchd deployment, SQLite backup addressing, normal restart preflight

## Context

The launchd backup/recovery work in PR #1097 bootstrapped a retained predecessor plist and then copied its contents to the canonical path, deleting the loaded source. Normal restart deliberately requires the loaded plist and canonical plist to identify the same file. The copied inode and removed source violated that contract despite identical bytes and a healthy recovered process. Coordinator validation reproduced the restart-preflight rejection with the actual validator and mocked loaded-path observation; no live-launchd reproduction is claimed.

The same review demonstrated that interpolating a filesystem path into a SQLite `file:` URI interpreted `#`, `?` and percent escapes as URI syntax. Capacity, writer reservation and backup could address a different file and appear to verify an empty database.

## Options considered

1. Keep private-path bootstrap and weaken restart validation to compare bytes: rejected because identical contents do not establish loaded configuration identity.
2. Retain a permanent private hardlink or alias: rejected because it adds lifetime and cleanup obligations.
3. Publish and bootstrap the unchanged canonical plist: selected; it composes with the existing restart contract.

## Decision

Qualify ADR-263's predecessor recovery ordering: after positively establishing unchanged predecessor identity and preboundary recovery eligibility, durably publish its canonical plist before bootstrapping that canonical path. Verify the new PID and exact identity afterward. If publication, bootstrap or verification fails, attempt teardown and requarantine persistence, retaining truthful uncertainty and ownership. The temporary availability of the positively verified unchanged predecessor's startup configuration is acceptable before the candidate boundary. This does not authorize predecessor startup after an attempted or uncertain candidate boundary. Recovery teardown requires a returned recovery PID matching both the recovery observation and the final pre-bootout inspection; unknown or changed PIDs remain operator-owned uncertainty. Launchctl bootout targets a label, not an atomic PID-conditioned operation: maintenance exclusivity against external service managers remains necessary, and these checks do not promise atomic protection against a manager racing the final inspection.

Keep the normal restart same-file check unchanged; do not replace it with byte equality or retain a special alias solely for recovery. Use `Path.absolute().as_uri()` with explicit SQLite open modes at every filesystem-to-URI boundary.

## Consequences

A recovered predecessor composes with ordinary restart preflight without special recovery exceptions. URI-metacharacter and control-character paths are covered with real SQLite contents, missing-source behavior and writer fencing, rather than mocked connection arguments. Recovery is composed with the actual restart validator under mocked launchctl inspection. These tests do not constitute production adoption or a live-launchd restart qualification.

## Verification blind spots

The original fixtures used ordinary temporary paths and checked logical content only for those paths; their healthy empty-copy result did not falsify wrong-file addressing. Recovery tests checked runtime identity and plist bytes, but did not compose recovery output with normal restart's file-identity contract. Add both exact boundary regressions rather than a generalized test framework.
