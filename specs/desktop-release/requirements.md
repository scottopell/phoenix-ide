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

BEFORE retaining a successfully verified macOS architecture pair for preparation or release,
THE SYSTEM SHALL execute unchanged artifact copies on the matching native host with isolated home, state, database, temporary paths, authentication, and a fresh loopback port,
SHALL verify exact source/version/architecture and instance identity, normal authentication, graceful stop and listener closure, database integrity and foreign keys, and unchanged artifact digests,
AND SHALL retain only sanitized headless verification receipts as separate workflow artifacts, not additional public release assets.

## REQ-DESKTOP-REL-004 — Publish architecture-specific archives

WHEN preparing current `main` or publishing a desktop release,
THE SYSTEM SHALL build one desktop archive for Apple Silicon and one for Intel,
SHALL run each build on a matching macOS architecture,
AND SHALL reject an archive whose embedded helper lacks the required architecture.

## REQ-DESKTOP-REL-005 — Checksum the exact release set

THE direct-distribution release surface SHALL publish exactly the six standalone server artifacts, four browser CLI artifacts under REQ-DESKTOP-REL-011, two desktop archives, and `SHA256SUMS`,
AND `SHA256SUMS` SHALL contain exactly one matching SHA-256 digest for each of the twelve non-manifest assets.

WHEN retrying a historical tagged source without the browser CLI entrypoint,
THE SYSTEM SHALL retain that source's exact six-server/two-desktop artifact contract.

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

WHEN the immutable tagged source supplies reviewed release notes,
THE SYSTEM SHALL include those notes in the request that makes the verified private release public,
AND SHALL NOT rely on a subsequent edit of a public release to disclose acceptance limitations.

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

WHEN a release workflow is running,
THE SYSTEM SHALL prevent another release workflow from concurrently creating a tag or mutating release publication.

IF GitHub replaces a pending workflow because another release request enters the same concurrency group,
THEN THE SYSTEM SHALL expose that canceled run in Actions history for operator inspection and explicit disposition after the active workflow completes: resume it only when its exact tag and version remain valid, otherwise retain its safe stale-version refusal.

WHEN creating a new release tag,
THE SYSTEM SHALL require its supported version to follow every existing supported release tag.

WHEN publishing a stable draft,
THE SYSTEM SHALL refuse to make it latest if a newer stable release is already latest.

WHEN retrying an existing exact release tag,
THE SYSTEM SHALL preserve the historical tagged build identity while using the protected workflow's current publication verifier.

## REQ-DESKTOP-REL-010 — Protected preparation without release mutation

WHEN an authorized operator requests desktop artifact preparation from `main`,
THE SYSTEM SHALL bind preparation to the exact current `main` commit and source version, run both supported macOS architectures through the same protected Developer ID signing, notarization, stapling, Gatekeeper, embedded-helper, and checksum verification path used for release artifacts, and retain the resulting artifacts and sanitized verification receipts as GitHub Actions artifacts that are not attached to or published as a GitHub Release.

The preparation producer and paired launchd consumer under `specs/launchd-deployment/requirements.md` REQ-LDD-017 SHALL share receipt schema `1`: operation `prepare-main`, exact full source commit and version, host target, exact standalone basename and SHA-256, and checks for Developer ID signature, hardened runtime, accepted notarization with submission UUID, validated stapled ticket, accepted Gatekeeper, and identical embedded helper bytes. Local receipt validation SHALL NOT authenticate workflow origin; the operator SHALL establish the protected run/source association independently.

WHILE executing preparation,
THE SYSTEM SHALL NOT create or move a tag, create or mutate a GitHub release, run release publication, deploy, or install the prepared artifacts.

The preparation identity SHALL NOT authorize a release version, release tag, stable/RC classification decision, or byte-identical future rebuild. Public release SHALL continue to require the separately authorized version-bump/tag/publication flow.

## REQ-DESKTOP-REL-011 — Publish standalone browser CLI binaries

WHEN the immutable release source includes the browser CLI entrypoint,
THE SYSTEM SHALL build and publish `phoenix-browser-aarch64-apple-darwin`, `phoenix-browser-x86_64-apple-darwin`, `phoenix-browser-aarch64-unknown-linux-musl`, and `phoenix-browser-x86_64-unknown-linux-musl` from that same exact tagged commit,
SHALL include those bytes in the exact private/public asset and checksum verification,
AND SHALL execute browser-free help and schema discovery on each matching native build host before publication.

BEFORE publishing a macOS browser CLI binary,
THE SYSTEM SHALL sign it with the configured Developer ID Application authority, hardened runtime, and secure timestamp,
SHALL strictly verify the signature and configured team,
AND SHALL require accepted Apple notarization of the signed bytes.

Browser CLI publication SHALL NOT change the protected desktop-only preparation asset or receipt contract.

## REQ-DESKTOP-REL-012 — Exact ARM packaging incident preparation

WHEN an authorized operator selects `prepare-incident-237100-arm64` from the protected `main` workflow,
THE SYSTEM SHALL prepare only `aarch64-apple-darwin` from immutable candidate `237100c79d9ce672867b284167a099fa0b7a2bcf`,
SHALL verify that candidate is an ancestor of `origin/main`, its source version is exactly `0.13.0-rc.4`, and its complete `crates/phoenix-db/src/migrations.rs` git object has SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`,
AND SHALL reject candidate, target, or release-tag selection inputs.

The controller SHALL come from the exact workflow commit, distinct from the candidate, and SHALL materialize outside the candidate workspace after its clean native build. Candidate checkout, packaging source, embedded identity, and receipt schema `1` SHALL remain unchanged. A separate sanitized provenance artifact SHALL record the operation, full controller and candidate commits, target, workflow run and attempt, and retry policy. Its operation SHALL distinguish this single-architecture preparation from normal `prepare-main`; the unchanged packaging receipt SHALL NOT alone establish incident workflow origin.

Only exact `xcrun stapler staple <app>` and `xcrun stapler validate <app>` invocations for the candidate packager's absolute ARM `Phoenix.app` path under `RUNNER_TEMP` SHALL receive retries. The wrapper SHALL delegate every other invocation, including notarization and credential-bearing arguments, unchanged to absolute `/usr/bin/xcrun` without adding argument logging. It SHALL suppress stapler child output and emit only static retry/failure messages. Only service response code `68` SHALL allow at most four attempts with delays of 10, 30, and 60 seconds, a 45-second command timeout and a 300-second execution/retry deadline per invocation. The nominal maximum scheduled command time plus retry delays SHALL be 280 seconds (`4 × 45 + 10 + 30 + 60`). Other failures, including invalid ticket data code `65`, SHALL fail immediately. Timeout or cancellation SHALL SIGKILL the child process group and reap the child before failing packaging; timeout SHALL NOT itself authorize retry. Mandatory reaping can exceed the execution/retry budget when the kernel cannot immediately terminate a process; the budget SHALL NOT imply an absolute wall-clock termination guarantee. The protected packaging step SHALL have a 30-minute runner timeout as an outer guard, without shortening the native build step.

The incident SHALL retain every candidate-owned signature, accepted notarization, staple validation, Gatekeeper, embedded-helper, checksum, protected-material cleanup and native headless acceptance gate before retaining qualified payload artifacts. It SHALL NOT create or move tags, mutate or publish GitHub Releases, deploy, install, or expand compatibility admission under `specs/compatibility/requirements.md` REQ-COMP-015. Normal `prepare-main` and release operations SHALL continue to require both macOS architectures without this wrapper.

## REQ-DESKTOP-REL-013 — Exact Linux artifact incident preparation

WHEN an authorized operator selects `prepare-incident-237100-linux-x86_64` from the protected `main` workflow,
THE SYSTEM SHALL prepare only `x86_64-unknown-linux-musl` from immutable candidate `237100c79d9ce672867b284167a099fa0b7a2bcf`,
SHALL verify that candidate is an ancestor of `origin/main`, its source version is exactly `0.13.0-rc.4`, and its complete `crates/phoenix-db/src/migrations.rs` git object has SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`,
AND SHALL reject candidate, target, or release-tag selection inputs.

The operation SHALL build from a clean checkout of that exact candidate using the controller from the exact protected workflow commit, distinct from the candidate. It SHALL verify the resulting artifact is the exact nonempty statically linked ELF64 x86-64 asset with embedded version `0.13.0-rc.4` and full candidate commit identity. Its Actions artifact SHALL contain exactly two files: `phoenix_ide-x86_64-unknown-linux-musl-prepared-237100c79d9c` and `LINUX-PREPARATION-RECEIPT-x86_64-unknown-linux-musl.json`.

The schema-1 receipt SHALL contain exactly these top-level fields: `schema` (`1`), `operation` (`prepare-incident-237100-linux-x86_64`), `target` (`x86_64-unknown-linux-musl`), `version` (`0.13.0-rc.4`), `commit` (the full candidate commit), `controller_commit` (the distinct full workflow commit), `registry_sha256` (the complete candidate registry digest above), `run_id` and `run_attempt` (positive integers), `event` (`workflow_dispatch`), `ref` (`refs/heads/main`), `sha256` (an object with only the prepared artifact basename mapped to its lowercase SHA-256), and `checks` (an object containing only `clean_source`, `static_elf`, and `embedded_identity`, each equal to `verified`). The receipt SHALL be generated only after verifying the clean source, ELF architecture/static linkage, and embedded identity; no other field, file, checksum manifest, or runtime data is part of this payload.

This operation SHALL NOT create or move tags, mutate or publish GitHub Releases, deploy, install, or expand compatibility admission under `specs/compatibility/requirements.md` REQ-COMP-015. It SHALL NOT authorize any other candidate, version, Linux architecture/target, operation, or ordinary preparation/release behavior. Its Linux target does not alter REQ-DESKTOP-REL-012's ARM-only operation or the normal two-architecture macOS preparation/release requirements.

In `specs/compatibility/requirements.md`, this prepared artifact is not a compatibility-admitted source or authorization for bare-Linux activation, migration, downgrade, rollback, or any deployment facility.
