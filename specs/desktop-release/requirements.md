# Phoenix macOS Desktop Release Requirements

## REQ-DESKTOP-REL-001 — Publish a paired desktop artifact

WHEN Phoenix publishes a stable release for a supported macOS architecture,
THE SYSTEM SHALL publish a Phoenix.app archive for that architecture
AND SHALL embed the exact same checksummed `phoenix_ide` artifact published separately for that architecture
AND SHALL require the embedded binary's version and full Git commit to match the release tag and tagged commit.

## REQ-DESKTOP-REL-002 — Preserve standalone server artifacts

THE SYSTEM SHALL publish desktop archives as additional release assets
AND SHALL preserve these standalone server assets: `phoenix_ide-aarch64-apple-darwin`, `phoenix_ide-x86_64-apple-darwin`, `phoenix_ide-x86_64-unknown-linux-musl`, and `phoenix_ide-aarch64-unknown-linux-musl`,
AND SHALL preserve these debug-symbol assets: `phoenix_ide-x86_64-unknown-linux-musl-debug` and `phoenix_ide-aarch64-unknown-linux-musl-debug`,
AND SHALL preserve each named asset's executable or debug-symbol content contract.

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

## REQ-DESKTOP-REL-007 — Retry the exact release by converging the published set

IF a release tag already points at the exact commit being built,
THE SYSTEM SHALL permit the release jobs to rebuild and republish that same asset set
AND SHALL replace assets in the existing GitHub release without changing the tag or release identity
AND SHALL perform same-tag retry publication as a serialized sequence that cleans stale stage names and commits asset replacements one-by-one
AND SHALL tolerate an interrupted retry leaving the release temporarily mixed between old and new asset names or contents
AND SHALL make the next retry converge the release back to the exact required asset and checksum set for that tag and commit
AND SHALL treat publication as successful only when the release contains exactly the required assets and matching checksums for that tag and commit.

IF the version tag points at a different commit,
THE SYSTEM SHALL refuse to rebuild or overwrite that release from the current commit.
AND THE SYSTEM SHALL NOT promise local or durable rollback to the previously published asset set after an interrupted same-tag retry.

## REQ-DESKTOP-REL-008 — Stamp release-specific app versions truthfully

WHEN packaging a desktop release for tag `vX.Y.Z`,
THE SYSTEM SHALL pass `MARKETING_VERSION=X.Y.Z` into `xcodebuild`
AND SHALL pass a deterministic release-specific dotted `CURRENT_PROJECT_VERSION` whose positive major component equals `X+1` and has at most four digits, and whose minor and patch components equal `Y` and `Z` and each have at most two digits
AND SHALL verify the built app's Info.plist carries those exact resolved values before publication.
