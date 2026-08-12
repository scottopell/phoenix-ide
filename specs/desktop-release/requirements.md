# Phoenix macOS Desktop Release Requirements

## REQ-DESKTOP-REL-001 — Publish a paired desktop artifact

WHEN Phoenix publishes a stable release for a supported macOS architecture,
THE SYSTEM SHALL publish a Phoenix.app archive for that architecture
AND SHALL embed the exact same checksummed `phoenix_ide` artifact published separately for that architecture
AND SHALL require the embedded binary's version and full Git commit to match the release tag and tagged commit.

## REQ-DESKTOP-REL-002 — Preserve standalone server artifacts

THE SYSTEM SHALL publish desktop archives as additional release assets
AND SHALL preserve every existing standalone server and debug-symbol asset name and content contract.

## REQ-DESKTOP-REL-003 — Sign and notarize distributable apps

BEFORE publishing a Phoenix.app archive,
THE SYSTEM SHALL sign the standalone macOS `phoenix_ide` helper exactly once with the configured Developer ID identity,
SHALL embed those exact signed helper bytes into the app archive without mutating them,
SHALL sign the outer app with the app's hardened-runtime entitlements,
SHALL verify the complete signature,
SHALL obtain Apple notarization,
SHALL staple and validate the notarization ticket,
AND SHALL reject the release when signing or notarization credentials are unavailable or any verification fails.

## REQ-DESKTOP-REL-004 — Publish architecture-specific archives

THE SYSTEM SHALL build one desktop archive per supported macOS server architecture
AND SHALL run each build on a matching macOS architecture
AND SHALL reject an archive whose embedded helper does not contain that architecture.

## REQ-DESKTOP-REL-005 — Checksum the complete release set

THE SYSTEM SHALL include every desktop archive in the release `SHA256SUMS`
AND SHALL publish the desktop archives and checksum file in the same GitHub release as the standalone server assets.

## REQ-DESKTOP-REL-006 — Keep packaging locally verifiable

THE SYSTEM SHALL provide an unsigned local-test mode that verifies app construction, embedded-helper identity, architecture, built Info.plist version fields, exact embedded-helper bytes, temporary-directory fallback behavior, and archive naming without weakening the signed release path.

## REQ-DESKTOP-REL-007 — Retry the exact release safely

IF a release tag already points at the exact commit being built,
THE SYSTEM SHALL permit the release jobs to rebuild and republish that same asset set
AND SHALL replace assets in the existing GitHub release without changing the tag or release identity
AND SHALL preserve or restore the previously published asset set if republishing fails before the replacement set is fully uploaded.

IF the version tag points at a different commit,
THE SYSTEM SHALL refuse to rebuild or overwrite that release from the current commit.

## REQ-DESKTOP-REL-008 — Stamp release-specific app versions truthfully

WHEN packaging a desktop release for tag `vX.Y.Z`,
THE SYSTEM SHALL pass `MARKETING_VERSION=X.Y.Z` into `xcodebuild`
AND SHALL pass a deterministic release-specific integer `CURRENT_PROJECT_VERSION`
AND SHALL verify the built app's Info.plist carries those exact resolved values before publication.
