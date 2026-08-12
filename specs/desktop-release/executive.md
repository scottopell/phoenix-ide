# Phoenix macOS Desktop Release

Phoenix release automation publishes architecture-specific, signed, notarized Phoenix.app zip archives alongside the existing standalone server binaries. Each app consumes the already-built standalone macOS server artifact for its architecture, so the release contains one checksummed server binary represented both directly and inside the paired desktop host.

## Requirement mapping

| Requirement | Implementation and verification |
|---|---|
| REQ-DESKTOP-REL-001 | `build.rs` now embeds the full 40-character Git SHA; `build-macos-desktop` consumes the signed `build-macos` helper artifact and `package-desktop-release.sh` verifies embedded version plus full commit identity. |
| REQ-DESKTOP-REL-002 | Existing Linux/macOS server jobs and asset names remain unchanged; desktop archives are additional artifacts. |
| REQ-DESKTOP-REL-003 | `build-macos` imports the Developer ID certificate and signs the standalone helper once; `build-macos-desktop` verifies that signature, embeds byte-identical helper bytes, signs the app with hardened runtime, notarizes, staples, validates, and assesses the app. Missing credentials fail the job. |
| REQ-DESKTOP-REL-004 | Release matrices use matching arm64 and Intel runners; packaging verifies the helper architecture before and after embedding. |
| REQ-DESKTOP-REL-005 | Publish requires both desktop zips and includes all eight binary/archive assets in `SHA256SUMS`. |
| REQ-DESKTOP-REL-006 | `test-package-desktop-release.sh` exercises unsigned construction, identity matching, Info.plist version verification, TMPDIR fallback derived-data creation, exact helper-byte embedding, and archive naming with hermetic tool fixtures. |
| REQ-DESKTOP-REL-007 | The gate retries only when the existing tag points at the exact workflow commit; publish stages backups of any existing assets and restores them if a replacement upload fails before completion. |
| REQ-DESKTOP-REL-008 | `package-desktop-release.sh` derives `MARKETING_VERSION` from the release tag, computes a deterministic integer `CURRENT_PROJECT_VERSION`, passes both to `xcodebuild`, and verifies the built Info.plist before publishing. |

## Release secrets

The repository release environment must provide:

- `MACOS_CERTIFICATE_P12_BASE64`
- `MACOS_CERTIFICATE_PASSWORD`
- `MACOS_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_TEAM_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`

These secrets are consumed only by the macOS desktop packaging jobs. Existing standalone server publication does not use them.
