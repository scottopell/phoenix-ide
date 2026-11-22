# ADR-273: Fixed RC9 preparation without publication authority

- **Status:** Accepted
- **Date:** 2026-11-22
- **Affects:** REQ-DESKTOP-REL-014; specs/desktop-release/desktop-release.allium near FrozenRc9PreparationRequested

## Context

The qualified backup-verification helper needs an independently qualified frozen application successor. The commissioned scope retains RC7 application bytes, apart from the helper and two own-package version fields, plus qualification tests and their CI job. RC8 is immutable; its source and protected preparation evidence cannot establish RC9 qualification. The instruction to prepare the successor does not authorize tagging, publication, or production activation.

## Options considered

1. Repin RC8's operations: rejected because it would change their identity and evidence meaning.
2. Copy the release framework for RC9: rejected because signing, native acceptance and artifact policy would diverge.
3. Extend the existing preparation controller with an independently fixed RC9 profile and explicit preparation-only operation: chosen.
4. Admit publication simultaneously: rejected because RC9 has no completed protected preparation run to pin.

## Decision

Add only `prepare-frozen-rc9`, sharing the existing preparation verifier, latest exact manual-CI admission, external controller materialization, and protected two-architecture signing/notarization/native acceptance path. Its fixed identity is candidate `35ee3cacf590e755d62b88ed4f140b7ae4d7acc8`, tree `57fa3c14e2830d06a629026086268c2e96279cd0`, version `0.13.2-rc.9`, helper SHA-256 `ef126688b3b194bcb3ee7466011be66858a72f40a1f71b0d497825d9594fdb3e`, and ref `candidate/rc9-helper-qualification`.

The controller verifies sole direct RC7 parentage, RC7 application-base ancestry on main, the exact nine-path scope, and version-only bytes. The immutable tree is the complete byte/mode authority. RC8's commands, constants, qualification and publication pins remain unchanged. There are no arbitrary source, version or run inputs. RC9 provenance identifies its operation separately from the unchanged schema-1 packaging receipt.

This extends only the preparation exception in ADR-267 and reuses the preparation mechanism associated with ADR-270. It does not extend ADR-270's RC8 publication exception. A future fixed RC9 publication admission requires its own completed protected preparation evidence and separately reviewed normative change. No tag, release mutation, deployment, installation or compatibility authority is conferred.

## Consequences

Source/controller qualification, protected native preparation, publication, and fresh backup/recovery rollout review remain distinct evidence boundaries. Failed or pending latest exact-candidate CI refuses preparation even if older or helper-only runs passed. The implementation can be reviewed before protected preparation is dispatched; its presence is not authorization to dispatch it.
