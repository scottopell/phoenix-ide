# Phoenix macOS Desktop Release

Phoenix release source contains a direct-distribution path for architecture-specific Phoenix.app ZIP archives alongside the six standalone server/debug artifacts. The source is under qualification; real Developer ID signing, Apple notarization, environment configuration, and clean-host acceptance remain external readiness gates.

## Requirement mapping

| Requirement | Implementation and verification |
|---|---|
| REQ-DESKTOP-REL-001 | The macOS release matrix builds the standalone helper and app from one immutable checkout. `package-desktop-release.sh` resolves the unchanged clean embedded Git identity to the full gate commit and compares helper bytes before and after outer-app signing. |
| REQ-DESKTOP-REL-002 | Linux and macOS standalone names remain unchanged. The two desktop ZIPs are additions produced by the existing macOS architecture matrix. |
| REQ-DESKTOP-REL-003 | The protected macOS job imports a Developer ID certificate, verifies configured certificate authority, signs the helper once, packages and signs the app with hardened runtime, then submits, staples, verifies, and assesses it. Real signing/notarization evidence is not yet available. |
| REQ-DESKTOP-REL-004 | The release matrix uses matching Apple Silicon and Intel runners; packaging verifies the helper architecture. |
| REQ-DESKTOP-REL-005 | Publish enumerates eight payload assets, creates `SHA256SUMS`, and verifies exact draft and public names plus GitHub-reported SHA-256 digests. |
| REQ-DESKTOP-REL-006 | `test-package-desktop-release.sh` covers unsigned shell orchestration with tool doubles. The `macos-app` hosted workflow also builds a real unsigned Release app, packages it through `package-desktop-release.sh`, and verifies the resulting archive on macOS. |
| REQ-DESKTOP-REL-007 | Every retry rebuilds and validates both architecture pairs from the immutable tagged commit before `publish-release-assets.sh` compares them with any public release. The publisher creates or resumes a private exact-tag draft, verifies it remains private before each mutation, replaces timestamp-dependent draft assets, verifies the complete exact set, and only then makes it public. It never deletes the release itself; an exact public release is unchanged and an inexact public release fails closed without replacement. |
| REQ-DESKTOP-REL-008 | The shared release-version parser derives numeric Apple marketing/build versions for stable and bounded RC SemVer; the gate rejects unrepresentable versions before tag creation, and packaging verifies both exact plist values while preserving complete SemVer in helper identity. |
| REQ-DESKTOP-REL-009 | The shared release-version parser accepts only stable or bounded `rc.N` SemVer, drives workflow/publisher channel metadata, keeps RCs non-latest, and fails closed on stable/RC metadata disagreement. RCs use the same protected signing, notarization, exact-asset, and private-draft path as stable releases. |

## Proposed protected configuration

ADR-059 is Proposed. Its reuse of the Paperclip-proven TeamIdentifier/API-key mechanics and its Phoenix-specific private-draft publication choice require normative review before release activation.

The proposed `macos-release-signing` GitHub Environment configuration is:

Secrets:

- `DEVELOPER_ID_P12_BASE64`
- `DEVELOPER_ID_P12_PASSWORD`
- `APPLE_TEAM_ID`
- `APP_STORE_CONNECT_ISSUER_ID`
- `APP_STORE_CONNECT_KEY_ID`
- `APP_STORE_CONNECT_API_KEY` (raw PEM text)

No release-specific GitHub variables are required by the proposed source.

The environment policy, values, and approval rules are external configuration. Source presence does not establish that they are configured or authorize access to them.

## Excluded scope

This direct-distribution path does not add Mac App Store packaging, DMG or installer production, Sparkle, a desktop updater, or changes to managed launchd/systemd/bare deployment ownership.
