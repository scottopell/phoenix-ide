# Publish the qualified ProductConversation milestone release candidate

## Authority and boundary

Standing user authority, coordinated by Global, permits ProductConversation milestone RC creation/publication through the supported pipeline. Durable coordinator record: https://github.com/scottopell/phoenix-ide/issues/806#issuecomment-5982664150 . Global remains final PR merge owner. This does not authorize a stable release, App Store/TestFlight publication, implicit devmbp upgrade, or model/config changes.

## Version selection evidence

At selection time, authoritative remote history showed latest stable `v0.12.0`, no RC, and no collision/open bump PR for the proposed version. Supported `release_version.py validate-new-from-tags` rejects `0.12.1-rc.1`: REQ-DESKTOP-REL-008 requires RC versions at or above `0.13.0` for monotonic Apple build-number ordering. The supported candidate is `0.13.0-rc.1`, with Apple marketing version `0.13.0` and build `1.13.1`. Recheck remote tags/releases/bump PRs immediately before creating the bump; selection is not publication.

## Gates and acceptance

- PR #836 controller/ADR-075 must qualify and land through Global; no bump/tag before that declared source gate.
- Open the version bump using the supported helper from exact fresh main; qualify its exact source and hand to Global for merge.
- Observe the release workflow's actual immutable tag/build source and both macOS signing/notarization receipts, Linux payloads, private draft checks, and public verifier.
- Require the exact nine-name release set (eight payloads plus SHA256SUMS), matching downloaded/GitHub digests, `prerelease=true`, and `isLatest=false`.
- Draft and verify release notes with the required AI-generated banner; preserve deployment compatibility warnings and do not claim universal rollback.
- Close only after recording actual release URL/tag/source/run/asset verification. No RC exists or was published at task creation.

## Independent receipts already established

Protected preparation run37220027105 qualified both macOS architectures at source `f5f98d7b2e51b112157e12c2b23d62c1a3c58d71`; it did not publish a release. Separately authorized devmbp transaction20261004T185609Z-8f1cdf96 activated that exact runtime using controllera31cddc3c0165606bb98bbb0f62fe8dfea2999a1; a future RC must not silently replace it.
