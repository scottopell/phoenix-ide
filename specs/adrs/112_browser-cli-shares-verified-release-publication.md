# ADR-112: Browser CLI shares verified release publication

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-DESKTOP-REL-005/009/010/011; `exact_inaccessible_assets_and_digests`

## Context

The standalone browser CLI is useful to coding harnesses without the Phoenix server. Requiring a Rust build limits adoption. The release workflow already builds four native targets, protects macOS signing authority, and verifies an exact private asset set before publication. Its exact-tag retry path uses current workflow tooling against historical source commits, some of which do not contain the CLI.

## Options considered

1. **Separate CLI releases:** independent versioning and tags would duplicate release authority and let browser/server artifacts refer to different source commits.
2. **Attach unsigned binaries after publication:** less workflow work, but weakens macOS usability and exposes a partial, changing public asset set.
3. **Extend the existing verified asset set:** reuse native targets, signing configuration, exact source selection, and private publication, while preserving the historical source's required asset set on retry.

## Decision

Build one size-optimized `phoenix-browser-<target>` executable for each existing target and publish it with the server and desktop artifacts from the same tagged commit. Add these four payloads to `SHA256SUMS` and the exact private/public verifier inputs. Run help and schema discovery on matching native hosts without launching Chrome or Phoenix.

Sign macOS browser bytes with the existing configured Developer ID Application authority and require strict team verification and accepted notarization before publication. A temporary submission archive is a notarization transport, not a release artifact. Browser binaries do not need separate symbol-rich variants.

Determine whether the immutable gated source contains the CLI entrypoint before selecting build and publication work. Historical exact-tag retries retain their original eight payloads. This preserves the existing retry contract rather than introducing executable or daemon compatibility. Keep `prepare-main` restricted to its existing desktop pair, receipt, and checksum contract.

## Consequences

- **Positive:** coding harnesses can download browser tools directly; checksums and publication prove one source and complete asset set.
- **Negative:** each release compiles four additional executables and makes two additional Apple notarization submissions. Release checks must cover both source capabilities and keep preparation separate.
- **Neutral:** browser installation is independent of Phoenix server deployment. Release channel, tag immutability, and private-public publication rules are unchanged.

## References

- [Desktop release requirements](../desktop-release/requirements.md)
- [Desktop release lifecycle](../desktop-release/desktop-release.allium)
- [ADR-065: Release candidates share the stable artifact path](065_release-candidates-share-the-stable-artifact-path.md)
- [ADR-107: Standalone browser uses a session daemon](107_standalone-browser-uses-session-daemon.md)
- `.github/workflows/release.yml`; `publish-release-assets.sh`; `verify-published-release.sh`
