# ADR-267: Publish the exact frozen RC7 candidate

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-DESKTOP-REL-002; `release-frozen-rc7`

## Context

The commissioned RC7 candidate is a version-only child of frozen application commit `d3b57573ccf8662d05125338aad2307bb03da598`, not a commit on current `main`. Requiring the candidate itself to be on `main` would make publishing this exact candidate impossible; selecting current `main` instead would silently include application changes that were not commissioned or reviewed for this release.

## Options considered

1. Require the candidate to be on `main` and abandon the exact frozen candidate.
2. Publish from latest `main`, accepting application drift beyond the commissioned version-only change.
3. Permit one fixed candidate only, while requiring its exact parent to remain an ancestor of `main` and proving the complete candidate tree is exactly the two version edits.

## Decision

Use the existing single-use frozen release controller and workflow operation for `release-frozen-rc7`, pinned to candidate `86071b4cc2ab0f9efdb8a3debf03b8569baf26f9`, base `d3b57573ccf8662d05125338aad2307bb03da598`, and tag `v0.13.2-rc.7`. Require successful exact-candidate manual CI, reject supplied tag input before candidate fetch, verify base ancestry and the exact sole-parent/two-file version-only tree, and refuse an existing tag. Keep the ordinary tagged-main rule unchanged for every other release operation. Existing signing, native acceptance, immutable tag/publication, and publisher gates remain in force.

## Consequences

- **Positive:** The signed release source is the exact commissioned application version without absorbing later `main` changes.
- **Negative:** This narrowly scoped release exception is not reusable for another version, candidate, base, or operation.
- **Neutral:** The candidate itself is not represented as a `main` commit. This decision grants no deployment, installation, backup, or compatibility authority.
- **Verification:** This record defines source-selection policy; hosted CI, signing/notarization, native acceptance, tag creation, and publication still require exact-run evidence.

## References

- [Desktop release requirements](../desktop-release/requirements.md), REQ-DESKTOP-REL-002.
- `scripts/frozen_rc5_release.py`: fixed candidate, base, exact tree, CI, and unused-tag gates.
- `.github/workflows/release.yml`: protected `release-frozen-rc7` operation.
