# ADR-084: Ordinary candidate exposure forbids predecessor restoration

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** REQ-LDD-019, REQ-PD-021, REQ-RU-008

## Context

Identity verification follows launchctl startup. A candidate can accept writes before its version endpoint is successfully verified, so the verified-commit checkpoint is too late to authorize predecessor restoration safely. Review also exposed that broadening ownership projection to systemd introduced an independent release/recovery workstream.

## Options considered

- Continue allowing manual predecessor restoration until verified commit: can discard writes accepted during the startup/verification window.
- Add a network gate or special server mode: expands the bounded deployment policy into a new runtime mechanism.
- Persist conservative exposure before startup, preserve data after possible exposure, and keep the feature launchd-only.

## Decision

The ordinary launchd helper durably records candidate exposure before launchctl startup. An explicit false exposure record permits the pre-exposure stopped/manual matched-restoration contract. Possible or unknown exposure forbids predecessor backup restoration and resume. Failed post-exposure activation remains stopped/fenced, preserving live data; candidate-only operator recovery needs independent qualification and no automatic recovery is implemented. Verified committed running candidates retain the existing publication-only finalization path.

All active ordinary claims fence controller deployment, restart and stop regardless of terminal state or pending bit until the owning helper/controller durably releases them. The new recovery/finalization/ownership wire projections are launchd-only. Systemd helper and other backend admission behavior remain baseline; no additional sudo permission or reconciliation API is introduced.

## Consequences

The policy sacrifices automatic availability recovery rather than risk accepted-write loss. An exposure checkpoint persisted before a startup error is intentionally conservative. This refines ADR-082's data-authority boundary and narrows ADR-083's projection scope without rewriting those historical decisions. Real host activation and candidate-only operator recovery remain separately qualified operations.
