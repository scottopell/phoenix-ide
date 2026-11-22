# ADR-271: Gate ordinary publication on exact merged-source CI

- **Status:** Accepted
- **Date:** 2026-11-22
- **Affects:** REQ-DESKTOP-REL-015; DesktopReleaseWorkflow release-tag admission

## Context

Version-file pushes start Release independently of CI. A squash merge produces a different commit from the reviewed PR head; that PR's success cannot qualify the exact versioned source. The signing environment does not supply a human reviewer hold. Existing fixed-candidate operations have deliberately different source gates.

## Options considered

1. Rely on green PR checks or signing-environment policy. This leaves the exact merged source unqualified.
2. Add an ordinary release controller or manual candidate selectors. This duplicates orchestration and expands the admission surface.
3. Wait in the existing read-only gate job for exact push/main CI. This preserves existing builds and publication while qualifying their source before any tag/publication mutation.

## Decision

Choose option 3. Ordinary new-version publication and exact-tag retry require the latest `ci.yml` push/main run at the resolved source SHA to complete successfully. Discovery and completion share a 3600-second deadline; API calls are bounded by the remaining time. Recheck run identity and attempt by ID before admission. Do not fall back to a different commit, PR event or workflow. Record selected run/attempt as gate outputs. Keep fixed-candidate and preparation paths unchanged.

Qualification is an initial per-Release-run admission snapshot. GitHub partial reruns may reuse a successful gate; rerunning the gate requalifies. We do not add per-job CI polling or continuous revocation when source CI is rerun after admission. This matches the existing retained-successful-job recovery path without introducing another release controller.

## Consequences

- **Positive:** Tags and publication cannot race ahead of exact merged-source FullCI.
- **Negative:** API outages, slow CI and historical tags without qualifying push/main evidence refuse admission. An operator must repair the exact-source qualification through a separately reviewed supported path; this gate offers no fallback selector.
- **Neutral:** A successful source gate does not qualify rebuilt artifacts, grant deployment authority, or replace signing, native acceptance and publisher verification.

## References

- `specs/desktop-release/requirements.md`, REQ-DESKTOP-REL-015
- `specs/desktop-release/desktop-release.allium`, `GateCreatesOnlyRepresentableReleaseTag` and `GateAcceptsOnlyExistingExactTag`
- `scripts/release_source_ci.py`, `qualify`
