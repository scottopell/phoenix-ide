# ADR-280: Enforce requested reserve at the launchd candidate boundary

- **Status:** Accepted
- **Date:** 2026-11-24
- **Affects:** REQ-LDD-002, REQ-LDD-004, REQ-LDD-029, REQ-LDD-033, REQ-COMP-021

## Context

The deployment operator needs an additional working reserve beyond the helper's bounded baseline. An operator sample before a long backup does not establish free capacity at candidate admission. The existing helper already owns this boundary and the SQLite writer reservation. Backup and configured database paths can be on different filesystems.

## Options considered

1. Keep an external operator capacity sample: no protocol change, but the observation can become stale before irreversible admission.
2. Bind the request in the existing helper manifest and check at its boundary: small supported-command change, with a deliberate protocol break.
3. Introduce a separate controller or disk reservation framework: substantially more lifecycle machinery without a demonstrated need.

## Decision

Use option 2. Protocol 4 requires `additional_reserve_bytes` as a nonnegative integer, including explicit zero. The CLI defaults to zero and forwards the value for ordinary and controller-mediated launchd deployment. Nonlaunchd deployment rejects nonzero requests.

Keep the existing backup/first-install preflights and add the request to them. After verified backup, freshly require baseline plus the request on both the backup and database filesystems under the existing writer reservation, before durable boundary attempt. First install checks its configured database filesystem. Persist the request, required threshold and available observation in private transaction proof before boundary attempt. Shortfall remains a preboundary failure with the existing recovery rules.

Reject older manifests and mismatched helper protocols before disruption; do not infer zero for missing fields or convert retained transactions. Restart protocol 2 is unchanged. This is a narrow extension of the operational admission in ADR-263, not a new upgrade, downgrade or availability guarantee.

## Consequences

- **Positive:** The owning helper, rather than optimistic operator timing, enforces the requested additional headroom at actual admission.
- **Negative:** Older deploy helpers and manifests cannot be mixed with protocol-4 installers; a newly qualified source/helper pair is required.
- **Neutral:** Free-space sampling does not prevent external filesystem consumption or guarantee migration peak space. Backup verification, candidate-owned migration, retained evidence and manual postboundary recovery remain unchanged.

## References

- [Launchd requirements](../launchd-deployment/requirements.md)
- [Compatibility requirements](../compatibility/requirements.md)
- ADR-230, ADR-263, ADR-265
- `launchd_prod_deploy`, `Manifest.load`, `activate`, `_reserve_before_boundary`, `_first_install_admission`
