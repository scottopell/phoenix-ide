# Phoenix macOS Desktop Release

Phoenix release automation publishes architecture-specific, signed, notarized Phoenix.app zip archives alongside the existing standalone server binaries. Each app consumes the already-built standalone macOS server artifact for its architecture, so the release contains one checksummed server binary represented both directly and inside the paired desktop host.

## Requirement mapping

| Requirement | Implementation and verification |
|---|---|
| REQ-DESKTOP-REL-001 | `build.rs` embeds the full 40-character Git SHA; `build-macos-desktop` consumes the signed `build-macos` helper artifact; and `package-desktop-release.sh` verifies embedded version plus full commit identity against the release tag and tagged commit. |
| REQ-DESKTOP-REL-002 | Existing Linux/macOS server jobs and asset names remain unchanged; desktop archives remain additional artifacts in the same release. |
| REQ-DESKTOP-REL-003 | `build-macos` imports the Developer ID certificate and signs the standalone helper once; `build-macos-desktop` verifies that signature, embeds byte-identical helper bytes, signs the outer app with hardened runtime, verifies the complete signature, notarizes, staples, validates, and assesses the app. Missing credentials or any signing/notarization failure fails the job. |
| REQ-DESKTOP-REL-004 | Release matrices use matching arm64 and Intel runners; packaging verifies the helper architecture before and after embedding. |
| REQ-DESKTOP-REL-005 | Publish requires both desktop zips and succeeds only when `SHA256SUMS` covers the exact required standalone and desktop asset set. |
| REQ-DESKTOP-REL-006 | `test-package-desktop-release.sh` exercises unsigned construction, prerelease-tag/full-SHA rejection, Info.plist version verification, bare tmp fallback, exact helper-byte embedding, and archive naming without changing the signed release path. |
| REQ-DESKTOP-REL-007 | Same-tag retries are gated on the release tag pointing at the exact workflow commit. Publication proceeds through serialized stage cleanup and one-by-one commit steps, so an interruption may leave a temporarily mixed release. The next retry is expected to remove stale stage names and converge the release back to the exact required asset and checksum set. The design no longer promises local or durable rollback to the previous published set. |
| REQ-DESKTOP-REL-008 | `package-desktop-release.sh` derives `MARKETING_VERSION` from the release tag, computes a deterministic integer `CURRENT_PROJECT_VERSION`, passes both to `xcodebuild`, and verifies the built Info.plist before publication. |

## Release secrets

The repository release environment must provide:

- `MACOS_CERTIFICATE_P12_BASE64`
- `MACOS_CERTIFICATE_PASSWORD`
- `MACOS_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_TEAM_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`

`MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, and `MACOS_SIGNING_IDENTITY` are shared by both macOS jobs because the standalone helper is signed before it is published and embedded. `APPLE_ID`, `APPLE_TEAM_ID`, and `APPLE_APP_SPECIFIC_PASSWORD` are consumed only by the desktop packaging jobs for notarization. Linux publication does not use these secrets.
