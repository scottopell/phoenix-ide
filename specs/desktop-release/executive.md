# Phoenix macOS Desktop Release

Phoenix release source contains a direct-distribution path for architecture-specific Phoenix.app ZIP archives alongside the six standalone server/debug artifacts. Protected preparation run 37220027105 qualified both architectures at source `f5f98d7b2e51b112157e12c2b23d62c1a3c58d71`: Developer ID signatures and hardened runtime verified, Apple notarization Accepted, staples validated, and Gatekeeper accepted. RC publication and clean-host desktop user acceptance remain separate gates.

## Requirement mapping

| Requirement | Implementation and verification |
|---|---|
| REQ-DESKTOP-REL-001 | The macOS release matrix builds the standalone helper and app from one immutable checkout. `package-desktop-release.sh` resolves the unchanged clean embedded Git identity to the full gate commit and compares helper bytes before and after outer-app signing. |
| REQ-DESKTOP-REL-002 | Linux and macOS standalone names remain unchanged. The two desktop ZIPs are additions produced by the existing macOS architecture matrix. |
| REQ-DESKTOP-REL-003 | The protected macOS job imports a Developer ID certificate, verifies configured certificate authority, signs the helper once, packages and signs the app with hardened runtime, then submits, staples, verifies, and assesses it. Real evidence is available from [preparation run 37220027105](https://github.com/scottopell/phoenix-ide/actions/runs/37220027105): Apple Silicon submission `57792d41-55ce-4302-911a-dd1479842f97` and Intel submission `9ec7336a-84d9-4f54-b2da-779350fd94f8` were Accepted; both staples and Gatekeeper assessments passed. |
| REQ-DESKTOP-REL-004 | The release matrix uses matching Apple Silicon and Intel runners; packaging verifies the helper architecture. |
| REQ-DESKTOP-REL-005 | Publish enumerates twelve payload assets when tagged source includes the browser CLI (eight for historical source without it), creates `SHA256SUMS`, and verifies exact draft/public names plus GitHub-reported SHA-256 digests. |
| REQ-DESKTOP-REL-006 | `test-package-desktop-release.sh` covers unsigned shell orchestration with tool doubles. The `macos-app` hosted workflow also builds a real unsigned Release app, packages it through `package-desktop-release.sh`, and verifies the resulting archive on macOS. |
| REQ-DESKTOP-REL-007 | Every retry rebuilds and validates both architecture pairs from the immutable tagged commit before `publish-release-assets.sh` compares them with any public release. The publisher creates or resumes a private exact-tag draft, verifies it remains private before each mutation, replaces timestamp-dependent draft assets, verifies the complete exact set, and only then makes it public. It never deletes the release itself; an exact public release is unchanged and an inexact public release fails closed without replacement. |
| REQ-DESKTOP-REL-008 | The shared release-version parser derives numeric Apple marketing/build versions for stable and bounded RC SemVer; the gate rejects unrepresentable versions before tag creation, and packaging verifies both exact plist values while preserving complete SemVer in helper identity. |
| REQ-DESKTOP-REL-009 | The shared release-version parser accepts only stable or bounded `rc.N` SemVer, drives workflow/publisher channel metadata, keeps RCs non-latest, and fails closed on stable/RC metadata disagreement. RCs use the same protected signing, notarization, exact-asset, and private-draft path as stable releases. |
| REQ-DESKTOP-REL-010 | The protected `prepare-main` workflow operation binds both production macOS signing/notarization jobs to exact current `main`, retains prepared artifacts plus sanitized receipts as GitHub Actions artifacts without attaching them to a GitHub Release, and structurally excludes tag creation, GitHub release mutation, publication, deployment, and installation. |
| REQ-DESKTOP-REL-012 | The fixed `prepare-incident-237100-arm64` operation gates candidate git-object version/registry/main ancestry, selects only ARM, and extracts the exact workflow controller outside the unchanged candidate workspace after native build. Its scoped wrapper retries only the matching temporary app staple/validate service failures under a bounded policy; separate sanitized provenance supplements, but does not alter, schema-1 receipts. Focused fixtures cover orchestration/privacy/process cleanup; real protected artifact production remains unverified until an exact hosted run passes. |

## Qualified protected configuration

ADR-063 records the protected TeamIdentifier/API-key and private-draft architecture. The deployed preparation path has passed both real macOS jobs; release publication is not inferred from preparation success.

The `macos-release-signing` GitHub Environment was exercised successfully by the protected preparation run. Its configuration contract is:

Secrets:

- `DEVELOPER_ID_P12_BASE64`
- `DEVELOPER_ID_P12_PASSWORD`
- `APPLE_TEAM_ID`
- `APP_STORE_CONNECT_ISSUER_ID`
- `APP_STORE_CONNECT_KEY_ID`
- `APP_STORE_CONNECT_API_KEY` (raw PEM text)

No release-specific GitHub variables are required by this path.

The environment policy, values, and approval rules are external configuration. Source presence does not establish that they are configured or authorize access to them.

## Private candidate headless verification

Both `prepare-main` and release packaging gate retention on the existing native-host signed-helper headless check in both macOS matrix jobs, after protected signing material is removed. `scripts/check-signed-helper.cjs` executes unchanged copies extracted from the mode-specific artifacts with isolated home/state/database/temp paths, private in-memory authentication and a fresh loopback port. It checks exact source/version/architecture and instance identity, normal authentication, graceful stop/listener closure, database integrity/foreign keys, and unchanged artifact hashes. Success and failure receipts are retained separately as `headless-acceptance-<target>-<commit>-<run>-<attempt>-success|failure` Actions artifacts, never runtime logs/database/cookies. Run/attempt qualification preserves earlier evidence on reruns; distinct outcome suffixes avoid a failure handler colliding with a receipt uploaded before a later payload-upload failure. Preparation also includes `HEADLESS-ACCEPTANCE-<target>.json` in its existing output bundle; its installation manifest remains exactly the standalone and app ZIP. Release payload artifacts also contain the separately verified browser CLI when the tagged source includes it. Public publication requires twelve payloads plus `SHA256SUMS`, or the historical eight-payload contract for tagged source without the CLI.

Regression fixtures exercise control flow and privacy boundaries, not actual Developer ID or candidate runtime acceptance. The headless receipt explicitly excludes GUI, provider, and install/update/rollback acceptance. Release builds must pass this same native check on their newly signed bytes; earlier preparation receipts do not qualify a rebuilt release. Missing dSYM retention is not a new gate. macOS GUI and physical-iPhone acceptance remain separately disclosed when user-deferred. No new candidate execution is claimed until its exact protected run completes.

`desktop-release.allium` models native acceptance as an explicit per-pair pending/passed/failed state. Verified private publication, public publication, exact public retries, and verified normal preparation require both architecture passes. The separate `IncidentPreparation` entity permits only the fixed ARM operation under REQ-DESKTOP-REL-012, not a partial pass of normal preparation. The native pass rule requires unchanged byte-paired artifacts, native isolation/identity/authentication/database/shutdown checks, protected-material cleanup, and sanitized receipt retention.

Publisher regression tests default the publisher child to `/bin/bash`, including macOS Bash 3.2. Publication uses a nonempty argument array containing the required draft/channel/latest fields, appending notes only when supplied, so no-notes runs do not expand an empty array under `set -u`.

## Cache-independent native acceptance workspace

Native workflow QA uses `qa-artifacts/release-qa/<run>/<attempt>/<target>` inside the workspace, outside the Cargo `target` cache. Each attempt receives a fresh runtime path without deleting previous proof or relaxing the tagged checker's freshness/path guards. Receipt upload paths match this non-hidden root and continue to retain only sanitized JSON, without depending on hidden-parent glob behavior. The workflow calls the unchanged tagged `checkSignedHelper` export; its wrapper logs only allowlisted static preflight failure codes, with a generic receipt-directed fallback for all other failures, never raw exception details or stacks.

The first RC2 release attempt (`38095463829`, immutable source `4f4c3bda16fd50e92ad5b9794bf3db2c97f7bd9f`) passed ARM64 packaging and protected-material cleanup, then failed before any sanitized receipt was initialized. Intel was canceled and publication skipped. Runner and Node metadata both identify ARM64; the job restored the full `target` cache. A restored empty runtime directory is a reproducible freshness hazard because the pinned cache cleanup preserves directory structure, but the hidden first-attempt error code does not prove that was its exact cause. This workflow repair isolates that hazard and exposes safe preflight codes without changing tagged checker or product bytes. A reviewed exact-tag retry still must rebuild and pass both native checks; the failed run supplies no release or deployment admission.

## Retry publication dependency gate

Publication explicitly requires a non-canceled workflow, a successful release-authorizing gate, successful Linux and macOS build matrices, and either successful tag creation or a skipped tag job when the gate sets `create_tag=false` for the existing immutable tag. This status-aware job condition avoids Actions' implicit `success()` skip propagation through the intentionally skipped tag job without allowing failed/canceled/skipped builds or tag failures to publish. Owning guards execute the actual condition across all 2,048 result/output/cancellation combinations.

Controlled retry `38098760226` used workflow source `84c33540c07998d7a2966a7a1b6f065d86e34f3b`, gated unchanged artifact source `4f4c3bda16fd50e92ad5b9794bf3db2c97f7bd9f`, and passed both native signing/notarization/headless checks plus both Linux builds. The tag job correctly skipped; the old publication condition nevertheless skipped publication through implicit ancestor status gating. Overall run success did not produce a GitHub Release. Both sanitized native receipts and ARM64 pair downloads are retained as exact-run evidence, not public-release or deployment admission. A reviewed workflow correction and controlled exact-tag retry remain required; no manual publisher bypass or retag is used.

## Reviewed notes before public visibility

The publisher reads optional `docs/releases/<tag>.md` notes from the immutable gated commit, while retaining current protected verifier tooling on historical retries. Requested notes must be a nonempty file before any remote API action. The existing final publication PATCH carries the notes body together with `draft=false` and exact channel/latest metadata, after private asset/tag verification. Releases without committed notes retain generated notes; already-public exact retries do not edit notes or assets. RC2 notes disclose the deferred macOS GUI and phone/Safari/PWA passes and distinguish native headless evidence from provider, GUI, published-updater, or universal rollback acceptance. This is publication plumbing, not an assertion that RC2 builds have run or passed.

## Excluded scope

This direct-distribution path does not add Mac App Store packaging, DMG or installer production, Sparkle, a desktop updater, or changes to managed launchd/systemd/bare deployment ownership.

## Standalone browser CLI release assets

REQ-DESKTOP-REL-011 and ADR-112 extend the exact publication set with one size-optimized `phoenix-browser-<target>` for each existing macOS/Linux target. The gate derives CLI availability from the immutable source, preserving historical exact-tag retries. Release-only macOS browser builds use the protected Developer ID identity, strict configured team/architecture/runtime/timestamp checks, and accepted Apple notarization of a temporary archive; native help/schema checks launch neither Chrome nor Phoenix. Preparation retains its existing desktop-only outputs. Actual protected browser signing/notarization and public artifact verification remain gates on the release run; local command doubles do not establish those passes.

## Exact ARM packaging controller repair

ADR-127 isolates `prepare-incident-237100-arm64` from normal current-main preparation and release. Candidate `237100c79d9ce672867b284167a099fa0b7a2bcf` remains version `0.13.0-rc.4` with registry SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`; neither source scripts nor embedded identity are patched. The exact workflow commit supplies only the temporary XCRUN controller after the clean native build. Normal operations retain both architectures and do not install the wrapper. The incident bundle retains `INCIDENT-CONTROLLER-PROVENANCE-aarch64-apple-darwin.json` beside the unchanged packaging/headless receipts and two-payload checksum manifest. Its operation and controller pin are required to interpret this as single-architecture incident evidence, not an ordinary two-architecture `prepare-main` run.

Host-confirmed evidence for both candidate ARM failures is `68` after notarization `Accepted`, with CloudKit `NSURLErrorDomain -1005` connection lost. The first run's initial staple/validate succeeded before a later separate validate failed with `68`. Only service-response `68` retries for both exact commands; invalid ticket data `65` and all other failures are immediate. Four attempts with 10/30/60-second delays and 45-second per-command limits schedule at most 280 seconds within a 300-second execution/retry budget per invocation. Timeout/cancellation SIGKILLs the child process group and reaps the child; mandatory reaping can exceed the budget for uninterruptible kernel sleep, so this is not an absolute wall-clock promise. A 30-minute protected-packaging step timeout supplies an outer runner guard without constraining the separate native build. This policy does not promise Apple ticket propagation or make a failed run qualified. No hosted dispatch, signing, publication, deployment, or installation is implied by the source repair.
