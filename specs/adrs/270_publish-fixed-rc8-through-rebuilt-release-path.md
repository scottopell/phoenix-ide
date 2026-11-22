# ADR-270: Publish fixed RC8 through the rebuilt release path

- **Status:** Accepted
- **Date:** 2026-11-21
- **Affects:** REQ-DESKTOP-REL-002, REQ-DESKTOP-REL-010, REQ-DESKTOP-REL-014

## Context

The commissioned helper-correction candidate is frozen RC8 `d5d2a45651035f58f3231f0dc82fd9ef59b7cb3c`, tree `5ebc83729fc5630f8ffcd03477b0aaac1187982b`, version `0.13.2-rc.8`, with deployment-helper SHA-256 `936886046a098896cb9656326d705776a66ed9d472161fdafc27f865c40f832e`. Publishing latest main would incorporate uncommissioned application changes. Preparation-only `prepare-frozen-rc8` deliberately has no tag or release authority. Its protected run `44975553897`, attempt 1, at controller `b2af8a67d592a415cdf16e13eaf72c1b648ff072` is still in progress in the supplied coordination evidence; qualification cannot be inferred from source checks or partial native progress.

## Options considered

1. Publish current main, accepting application drift beyond the frozen candidate.
2. Promote preparation payload bytes directly, requiring a separate artifact-promotion and byte-identity publication contract.
3. Admit only the fixed RC8 candidate after exact CI and protected preparation qualification, then rebuild through the existing release path.

## Decision

Choose option 3. Add only `release-frozen-rc8`, with no candidate, source, run, or tag selectors. Run the workflow from exact current main, require the immutable candidate ref and all source/tree/version/helper pins, RC8 ancestry from frozen RC7, and main ancestry from RC7's exact application parent. Require the latest exact-candidate manual CI run to be completed successful, rather than searching for an older success. Reject supplied tag input before candidate qualification; refuse an existing RC8 tag or a version not newer than supported release tags.

Require the fixed protected preparation run, attempt and controller above to finish successfully with the two nonexpired native payloads and exact success headless artifacts. An in-progress, failed, incomplete, unreadable or mismatched run is not eligible. This decision records a gate, not a claim that the run has passed.

After admission, tag only the frozen candidate and rebuild release bytes with the existing matrices and publisher. Signing/notarization, native acceptance on the newly built bytes, exact private-draft assets and digests, and non-latest RC metadata remain mandatory. Preparation evidence does not promise byte identity with a later build and cannot substitute for those checks. Keep `prepare-frozen-rc8`, normal-main preparation and incident preparation contracts separate and unchanged. ADR-267's RC7 exception is not generalized; ordinary releases retain their tagged-main rule.

## Consequences

- **Positive:** Publication can select the commissioned helper correction without later main application changes or a new publisher.
- **Negative:** Rebuilding repeats protected work; preparation success alone cannot publish, and missing or expired qualification artifacts block admission.
- **Neutral:** This is a fixed RC8 source-selection exception, not a general frozen-source or artifact-promotion facility.
- **Neutral:** No deployment, installation, compatibility, GUI, provider, or phone acceptance authority is granted.

## References

- [Desktop release requirements](../desktop-release/requirements.md), REQ-DESKTOP-REL-002/010/014.
- [Desktop release lifecycle](../desktop-release/desktop-release.allium), `FrozenRc8ReleaseRequiresFixedCompletedQualification`, `FrozenRc8ReleaseRefusesUnqualifiedRequests`, and `FrozenRc8PreparationRetainsNoPublicationAuthority`.
- [ADR-267](267_publish-exact-frozen-rc7-candidate.md).
- `scripts/frozen_rc8_release.py`: `gate`, `verify_preparation_qualification`, `verify_tag_unused`.
- `scripts/frozen_rc8_prepare.py`: `verify_candidate`, `latest_ci_run`.
- `.github/workflows/release.yml` and `scripts/publish-release-assets.sh`.
