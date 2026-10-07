# Qualify signed desktop artifacts without publication

Add a protected preparation-only entrypoint to the existing desktop release workflow that reuses the production macOS signing, notarization, stapling, Gatekeeper, checksum, and artifact path for an exact current-main commit without creating or moving a release tag and without running publication. Produce checkable run/artifact evidence, then surface the separate exact version/tag/publication decision.

## Acceptance criteria
- Preparation is main-bound and protected by `macos-release-signing`.
- It runs both macOS architectures through the production signing/notarization path.
- It cannot create a tag, publish a GitHub release, deploy, or install on devmbp.
- Prepared artifact identity is the exact full main commit; no version/tag authority is invented.
- Workflow/static tests enforce the non-publication boundary.
- A real protected run produces signing/notarization receipts and downloadable Actions artifacts for qualification.

## Current implementation

The typed `prepare-main` dispatch is isolated from release concurrency and uses a read-only gate plus the existing protected two-architecture macOS jobs. Repository write authority exists only in release-only tag/publication jobs. The package script emits command-derived sanitized receipts after accepted notarization, stapling, Gatekeeper, and helper-byte checks; the workflow adds and verifies portable SHA-256 manifests before retaining commit-qualified Actions artifacts for 14 days. These Actions artifacts are not confidential, are not attached to a GitHub Release, and do not authorize any version/tag/publication decision.

## Protected run evidence

- Run: https://github.com/scottopell/phoenix-ide/actions/runs/37108779892
- Exact source: `aa472b978b6b064c7d5c889547593cd41d07f27d` on `main`
- Operation: one `prepare-main` workflow dispatch
- Gate: succeeded and bound the run to the exact source commit
- Release mutation jobs: `tag-release`, Linux builds, and `publish` all skipped
- Intel job `111162481838`: standalone and app builds completed; strict codesign verification passed; Apple mapping was `MARKETING_VERSION=0.12.0`, `CURRENT_PROJECT_VERSION=1.12.0`; notarization request failed with HTTP 403 because a required Apple agreement is missing or expired
- Apple submission receipt: none; Apple rejected the request before creating an accepted submission identity/status
- Apple Silicon job `111162481811`: canceled by matrix fail-fast after the Intel notarization failure
- Protected-material cleanup: succeeded
- Retained artifacts: none; the workflow failed before artifact upload, so no unnotarized signed bytes were retained
- Tag/release/publication: unchanged; no tag, GitHub Release, publication, deployment, or installation occurred

## Qualified retry after Account Holder remediation

The user confirmed agreement acceptance. One subsequent protected preparation run succeeded; actual Apple receipts establish acceptance rather than inferring it from the account action.

- Run: https://github.com/scottopell/phoenix-ide/actions/runs/37220027105
- Exact source: `f5f98d7b2e51b112157e12c2b23d62c1a3c58d71` on `main`
- Operation: `prepare-main`; both macOS jobs succeeded; tag, Linux, and publisher jobs skipped
- Apple Silicon notarization: Accepted, submission `57792d41-55ce-4302-911a-dd1479842f97`
- Intel notarization: Accepted, submission `9ec7336a-84d9-4f54-b2da-779350fd94f8`
- Both receipts: Developer ID signature and hardened runtime verified, stapled ticket validated, Gatekeeper accepted, embedded helper bytes identical
- Portable checksum verification and artifact retention succeeded for both architectures
- Apple Silicon Actions artifact: `11309888709`, `desktop-preparation-aarch64-apple-darwin-f5f98d7b2e51b112157e12c2b23d62c1a3c58d71`
- Intel Actions artifact: `11310705834`, `desktop-preparation-x86_64-apple-darwin-f5f98d7b2e51b112157e12c2b23d62c1a3c58d71`
- Apple Silicon standalone SHA-256: `85c1146793c969ff90834c3faf96bc783c1a87151e5716de2b5291bb8b50147b`
- Apple Silicon app ZIP SHA-256: `015733f50b49d310fed77e704de37f70923bf4160fc2fadb342f311a6d6a4077`
- Intel standalone SHA-256: `5de4301ade85a17bdf6ee79f05e3a8989b559ac5d10dde0bcde8d605db477aeb`
- Intel app ZIP SHA-256: `edf99736cf5695070bb0c00422a85cab56145149f5d9d4d5224dee0200e2732b`

Preparation is complete. This run did not create a tag, publish a GitHub Release, deploy, or install. A separately authorized devmbp deployment subsequently consumed the qualified standalone under its own paired database/runtime safety contract; that operation is not preparation authority. RC publication remains a separate pipeline operation, not a claim about these preparation artifacts.
