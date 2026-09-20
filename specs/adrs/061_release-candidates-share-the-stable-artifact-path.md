# ADR-061: Release candidates share the stable artifact path

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** REQ-DESKTOP-REL-001, REQ-DESKTOP-REL-003, REQ-DESKTOP-REL-005, REQ-DESKTOP-REL-007, REQ-DESKTOP-REL-008, REQ-DESKTOP-REL-009; `ReleaseIdentity`, `ArchitecturePair`, `ReleasePublication`

## Context

Phoenix needed one bounded release-candidate channel for protected macOS artifact qualification before a final stable release. The existing release path already bound artifacts to an immutable tag and commit, signed and notarized both macOS architectures, staged an exact private draft, and made stable releases public only after verification.

Apple requires `CFBundleShortVersionString` and `CFBundleVersion` to contain numeric period-separated components. A SemVer suffix such as `-rc.N` therefore cannot be placed in either plist value. The existing stable bundle-build mapping also left no numeric space for distinct same-version candidates that sort below the final stable build.

## Options considered

1. **Qualify release candidates outside the release path** — avoids publication changes but cannot prove the protected signing, notarization, checksum, and immutable-identity path that stable releases use.
2. **Add a general prerelease-channel framework** — could model alpha, beta, RC, and promotion policies, but introduces channels and lifecycle behavior Phoenix does not need.
3. **Add one bounded RC form to the existing protected artifact path** — preserves one security and publication path while deriving classification from the validated version.

## Decision

Choose option 3.

Phoenix supports exactly two release-version forms: stable `X.Y.Z` and release candidate `X.Y.Z-rc.N`, where `N` is positive and bounded. The validated version structurally determines the channel; callers cannot supply an independent channel flag that disagrees with the tag.

Release candidates use the complete stable artifact path and security posture. They build the same architecture matrix, preserve the full RC SemVer in helper/release identity, sign with the protected Developer ID authority, require Apple-accepted notarization, staple and validate, pass Gatekeeper, produce the same checksum set, and remain private until exact verification succeeds.

GitHub publication sets release candidates to `prerelease=true` and `make_latest=false`. Stable publication sets `prerelease=false` and `make_latest=true`. Existing release metadata that disagrees with the validated version fails closed. Stable promotion is a new version and build through the normal version-bump flow; Phoenix does not relabel candidate bytes or move a tag.

For Apple numeric fields, `CFBundleShortVersionString` is the base `X.Y.Z`. Historical stable versions before `0.13.0` retain `CFBundleVersion=X+1.Y.Z`. Starting with the RC-capable range, `X.Y.Z-rc.N` maps to `X+1.Y.(Z*100+N)` and final stable `X.Y.Z` maps to `X+1.Y.(Z*100+99)`. Bounds reserve candidate numbers 1 through 98 and keep all components within the accepted numeric representation. This makes the mapping deterministic across architectures and immutable retries without a counter service.

`latest` deployment remains stable-only. An exact supported RC tag is an explicit opt-in and must match GitHub prerelease metadata, the full embedded RC SemVer, and the exact tagged commit. New versions must follow existing supported remote release tags, preventing a lower candidate or stable build from being published later. Manual retries rebuild from the immutable historical tag commit but execute publication with the protected workflow's current verifier, so verifier hardening remains effective across old tags.

## Consequences

- The first supported candidate version is `0.13.0-rc.1`; candidate syntax is not added below the numeric-mapping cutoff.
- Final stable bundle-build numbers at and after `0.13.0` use the reserved terminal slot rather than the historical patch component.
- RC publication cannot replace or mutate a differing public release and cannot become GitHub's latest release.
- No alpha/beta channel framework, release branch, scheduler, automatic promotion, updater behavior, deployment automation, or database compatibility guarantee is introduced.
- Protected signing/notarization execution and public publication remain separately authorized operations.

## References

- ADR-059: direct distribution uses protected signing and private drafts.
- ADR-060: modern build identity is full-length and legacy rollback is role-bound.
- `specs/desktop-release/requirements.md`
- `specs/desktop-release/desktop-release.allium`
- `.github/workflows/release.yml`
- `scripts/release_version.py`
- `scripts/tag-release.sh`
- `scripts/publish-release-assets.sh`
- `scripts/verify-published-release.sh`
- `macos/Phoenix/scripts/package-desktop-release.sh`
- `dev.py`
