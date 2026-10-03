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

## Remaining gate

The workflow change must land on default-branch `main` before an authorized `prepare-main` dispatch can produce real signed/notarized evidence. After landing, dispatch exact current `main`, download both architecture artifacts, independently verify receipt identities and checksums, and record the Actions run/artifact evidence here.
