# Phoenix macOS Desktop Release Requirements

## Scope

This specification owns direct-distribution packaging and publication of Phoenix macOS app archives and cumulatively refines the standalone release asset contract in `specs/production-deployment/requirements.md`.

It does not define Mac App Store packaging, DMG or installer production, Sparkle, a desktop updater, or managed launchd/systemd/bare activation.

## REQ-DESKTOP-REL-001 — Publish paired desktop artifacts

WHEN Phoenix publishes a stable release for a supported macOS architecture,
THE SYSTEM SHALL publish a Phoenix.app archive for that architecture,
SHALL embed the exact signed `phoenix_ide` bytes published separately for that architecture,
AND SHALL prove that the helper version and source identity resolve to the tagged commit.

## REQ-DESKTOP-REL-002 — Preserve standalone server artifacts

THE SYSTEM SHALL publish desktop archives as additions to the standalone server set,
SHALL preserve `phoenix_ide-aarch64-apple-darwin`, `phoenix_ide-x86_64-apple-darwin`, `phoenix_ide-x86_64-unknown-linux-musl`, and `phoenix_ide-aarch64-unknown-linux-musl`,
SHALL preserve `phoenix_ide-x86_64-unknown-linux-musl-debug` and `phoenix_ide-aarch64-unknown-linux-musl-debug`,
AND SHALL build every named artifact from one exact tagged main commit.

## REQ-DESKTOP-REL-003 — Sign and notarize distributable apps

BEFORE a Phoenix.app archive becomes public,
THE SYSTEM SHALL sign the standalone macOS helper once with a configured Developer ID Application identity,
SHALL verify that identity against configured release authority,
SHALL embed the exact signed helper bytes without mutating them,
SHALL sign the outer app with hardened runtime and a secure timestamp,
SHALL verify the complete signature,
SHALL obtain Apple notarization,
SHALL staple and validate the notarization ticket,
SHALL pass Gatekeeper assessment,
AND SHALL reject the release when protected configuration is unavailable or any check fails.

## REQ-DESKTOP-REL-004 — Publish architecture-specific archives

THE SYSTEM SHALL build one desktop archive for Apple Silicon and one for Intel,
SHALL run each build on a matching macOS architecture,
AND SHALL reject an archive whose embedded helper lacks the required architecture.

## REQ-DESKTOP-REL-005 — Checksum the exact release set

THE direct-distribution release surface SHALL publish exactly the six standalone artifacts, two desktop archives, and `SHA256SUMS`,
AND `SHA256SUMS` SHALL contain exactly one matching SHA-256 digest for each of the eight non-manifest assets.

## REQ-DESKTOP-REL-006 — Keep packaging locally verifiable

THE SYSTEM SHALL provide an unsigned test mode that verifies packaging orchestration, helper provenance, architecture, resolved Info.plist version fields, exact helper bytes, and archive naming without bypassing the signed release path.

## REQ-DESKTOP-REL-007 — Keep incomplete publication private

WHEN an exact-tag publication starts or resumes,
THE SYSTEM SHALL keep incomplete release contents inaccessible to consumers,
SHALL recover private publication work using the exact required asset and checksum set,
AND SHALL make the release public only after exact names, digests, tag identity, and commit identity verify.

IF an already-public release contains the exact required names and digests,
THE SYSTEM SHALL treat retry as an idempotent success without mutating the release.

IF an already-public release is incomplete or differs from the required set,
THE SYSTEM SHALL fail without replacing its assets or moving its tag.

IF the version tag identifies a different commit,
THE SYSTEM SHALL refuse the release from the current commit.

## REQ-DESKTOP-REL-008 — Stamp release-specific app versions truthfully

WHEN packaging a desktop release for a supported stable `X.Y.Z` or release-candidate `X.Y.Z-rc.N` version,
THE SYSTEM SHALL reject the release before tag creation unless `X < 9999`, `Y < 100`, `Z < 99`, and any release-candidate number satisfies `1 <= N <= 98`,
SHALL use `MARKETING_VERSION=X.Y.Z`,
SHALL map versions before `0.13.0` to the established numeric `CURRENT_PROJECT_VERSION=X+1.Y.Z`,
SHALL map `X.Y.Z-rc.N` to `CURRENT_PROJECT_VERSION=X+1.Y.(Z*100+N)`,
SHALL map final stable `X.Y.Z` versions at or after `0.13.0` to `CURRENT_PROJECT_VERSION=X+1.Y.(Z*100+99)`,
AND SHALL verify the built app carries both exact resolved values before publication.

The authoritative helper and release identity SHALL remain the complete SemVer, including `-rc.N`; numeric Apple bundle fields SHALL NOT replace it.

## REQ-DESKTOP-REL-009 — Bound release channels to validated versions

WHEN a release version is validated,
THE SYSTEM SHALL accept only stable `X.Y.Z` or release-candidate `X.Y.Z-rc.N` syntax with a positive bounded `N`
AND SHALL derive the release channel from that version.

WHEN a release candidate is prepared or retried,
THE SYSTEM SHALL produce the same complete signed, notarized, stapled, Gatekeeper-validated, checksummed artifact set as a stable release
AND SHALL keep the GitHub Release private until exact verification succeeds
AND SHALL publish it with `prerelease = true` and `make_latest = false`.

WHEN a stable release is prepared or retried,
THE SYSTEM SHALL publish it with `prerelease = false` and `make_latest = true`.

IF existing private or public release metadata differs from the channel derived from its validated version,
THEN THE SYSTEM SHALL fail without rewriting public metadata or mutating public assets.

Stable promotion SHALL use a new stable version and the normal version-bump release flow; the system SHALL NOT relabel release-candidate bytes as stable or move an existing tag.

WHEN creating a new release tag,
THE SYSTEM SHALL serialize tag-gate operations and require its supported version to follow every existing supported release tag.

WHEN publishing a release draft,
THE SYSTEM SHALL serialize publication across release tags.

WHEN publishing a stable draft,
THE SYSTEM SHALL refuse to make it latest if a newer stable release is already latest.

WHEN retrying an existing exact release tag,
THE SYSTEM SHALL preserve the historical tagged build identity while using the protected workflow's current publication verifier.
