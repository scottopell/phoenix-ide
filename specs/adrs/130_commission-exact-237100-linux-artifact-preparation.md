# ADR-130: Commission the exact 237100 Linux artifact operation

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-DESKTOP-REL-013; REQ-DESKTOP-REL-012 boundary unchanged; REQ-COMP-015 boundary unchanged

## Context

The protected release workflow contains a separately commissioned Linux artifact operation for the same immutable candidate fixed by the ARM packaging incident. The desktop-release contract describes the ARM operation as the only incident preparation and leaves the Linux operation undocumented. Leaving the Linux workflow without a matching normative contract makes its authorization, artifact identity and retained evidence ambiguous. The Linux path must remain a narrowly scoped preparation operation rather than a reusable Linux release or deployment path.

The candidate is `237100c79d9ce672867b284167a099fa0b7a2bcf`, version `0.13.0-rc.4`, with the complete migration registry SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`. The operation builds only `x86_64-unknown-linux-musl`; it validates the nonempty static ELF64 x86-64 binary and embedded version/full commit identity, then retains that binary and one schema-1 receipt as a closed two-file Actions artifact.

## Options considered

1. **Keep the ARM-only normative contract and treat Linux as incidental workflow implementation** — avoids adding an operation, but leaves authorization and the artifact receipt shape unspecified despite a runnable protected workflow path.
2. **Generalize incident preparation to multiple architectures or candidates** — makes future reuse easier, but expands the fixed exception and creates arbitrary candidate/target selection absent from the workflow.
3. **Specify an independent exact Linux operation for candidate 237100** — closes the contract/code gap while preserving the existing ARM-only requirement and normal macOS operations.

## Decision

Add REQ-DESKTOP-REL-013 and a separate `LinuxIncidentPreparation` behavior for only `prepare-incident-237100-linux-x86_64`, the fixed candidate/version/registry identity, the `x86_64-unknown-linux-musl` target, and the exact workflow controller. Retained evidence is exactly the prepared static ELF and its schema-1 receipt, with the receipt's exact provenance, one artifact digest, and three verified checks. Do not alter REQ-DESKTOP-REL-012: it remains ARM-only. Do not broaden normal preparation or release, admit any other candidate/target, or grant compatibility, deployment, installation, tag, or publication authority. This prepared binary is not an admitted bare-Linux activation source under REQ-COMP-015.

## Consequences

- **Positive:** The protected workflow has an explicit, checkable contract, including its closed receipt and artifact payload.
- **Negative:** The Linux operation remains intentionally non-reusable; another candidate, target or payload shape requires a separate decision and requirement.
- **Neutral:** Source-level fixtures do not establish that a hosted Linux artifact was produced or qualify deployment/compatibility behavior.

## References

- `specs/desktop-release/requirements.md` REQ-DESKTOP-REL-013
- `specs/desktop-release/desktop-release.allium` — `LinuxIncidentPreparation`
- `specs/compatibility/requirements.md` REQ-COMP-015
- `scripts/incident_stapler_controller.py` — `linux_gate_outputs`, `verify_linux_artifact`, `linux_receipt`
- `.github/workflows/release.yml` — `build-incident-linux`
- ADR-127 — fixed ARM operation boundary
