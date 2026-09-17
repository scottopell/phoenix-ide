# ADR-058: Direct distribution uses protected signing and private draft publication

- **Status:** Proposed
- **Date:** 2026-09-19
- **Affects:** REQ-DESKTOP-REL-003, REQ-DESKTOP-REL-005, REQ-DESKTOP-REL-007; `ArchitecturePair`, `ReleasePublication`

## Context

Phoenix needs a direct-distribution macOS release whose two app archives and six standalone artifacts share one immutable tag/main commit. Developer ID signing and Apple notarization require long-lived external authority, while GitHub Release publication can expose a partially uploaded set if assets are mutated after a release is public.

The release path must remain retryable after a transient build, notarization, or upload failure without moving the tag, publishing mixed bytes, or extending desktop distribution into managed installation ownership. Authentication and certificate-selection choices also need a reviewable boundary before any credential is configured.

## Options considered

1. **Human Apple ID credentials and subject-name signing selection** — supported by `notarytool` and simple to wire, but couples automation to a person's account lifecycle and permits ambiguous certificate-name selection.
2. **App Store Connect Team API key plus signed-code TeamIdentifier verification** — reuses the account/authentication mechanics proven by Paperclip's successful direct-distribution release, verifies the signed helper and app belong to the expected Apple team, and requires temporary key-file cleanup.
3. **Publish first and replace public assets during retry** — permits convergence after failure but temporarily exposes incomplete or mixed release contents.
4. **Prepare and verify a private draft before publication** — retries mutate only private state, and an already-public release is immutable; requires explicit draft recovery and exact digest verification.

## Decision

Propose option 2 for protected signing/notarization authority and option 4 for publication.

The macOS job uses one protected GitHub Environment. It imports a password-protected Developer ID Application certificate into an isolated random-password temporary keychain, derives the sole signing identity from that keychain, verifies the signed helper and app report the expected TeamIdentifier, and removes decoded files and the keychain after the job. Notarization uses the proven App Store Connect issuer/key/raw-PEM API authentication rather than a human Apple ID password, requires JSON status `Accepted`, and retrieves Apple's notarization log on failure.

Publication creates or resumes only a draft for the exact tag. The protected publisher is the sole authorized writer for that release while it confirms the release remains private before each mutation, replaces timestamp-dependent draft assets, verifies the complete exact names and digests, rechecks tag identity, and only then makes the draft public. It may delete private draft assets but never deletes the release itself. A public exact release is idempotent. A differing public release fails without mutation.

This proposal does not alter Phoenix's embedded build-identity format, managed deployment protocols, updater ownership, or release activation authority.

## Consequences

- **Positive:** Phoenix reuses release-account mechanics proven by a successful Paperclip Developer ID/notarization run; signed code is bound to the expected Apple team; retries cannot expose mixed public assets; exact public releases are immutable.
- **Negative:** The Phoenix repository still requires protected-environment provisioning and approval, sole-writer governance for release mutation, and GitHub Release draft/digest behavior for multi-asset recovery. GitHub offers no conditional publish operation, so that governance is part of the proposed authorization boundary.
- **Neutral:** The tag is still created by the version-bump workflow before protected signing jobs run. A downstream failure leaves an exact immutable tag and private or absent release that manual dispatch can retry.

## References

- ADR-017: production deployment shares preparation but keeps backend-owned activation.
- ADR-018: release updates use published release previews and approval-bound installations.
- ADR-034: compatibility guarantees are explicit and data-aware.
- `specs/desktop-release/requirements.md`
- `DesktopReleaseWorkflow`, `package-desktop-release.sh`, `publish-release-assets.sh`
- Apple `notarytool` authentication supports App Store Connect API keys and Apple ID app-specific passwords.
- Paperclip commit `05c3563cdb8d546a25a227b25975dc0a2b23c030`, successful run `32664159176`, and release `v1.0.2` establish the account/authentication prior art. Phoenix deliberately does not inherit Paperclip's sandbox assertion or public-release `--clobber` policy.
