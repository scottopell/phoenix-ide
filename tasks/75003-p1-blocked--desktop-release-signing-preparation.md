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

## Blocker

The Apple Developer/App Store Connect Account Holder must accept or renew the required legal agreement. This is an external Apple account gate, not a missing signing secret or repository defect. Do not retry unchanged. After the agreement is in effect, dispatch one new `prepare-main` run and require both architecture jobs, receipts, portable checksum verification, and commit identities to succeed before marking this task done.
