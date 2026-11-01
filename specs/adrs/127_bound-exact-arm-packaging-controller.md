# ADR-127: Bound the exact ARM packaging repair to a separate controller

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-DESKTOP-REL-004/010/012; IncidentPreparation; REQ-COMP-015 boundary unchanged

## Context

The launchd forward-replacement candidate is pinned to `237100c79d9ce672867b284167a099fa0b7a2bcf`, version `0.13.0-rc.4`, and the complete registry128 source digest. Protected ARM packaging needs a bounded ticket-service retry without changing that candidate's source or embedded identity. Normal preparation is intentionally tied to current main and both native architectures. A controller repair must not turn it into arbitrary historical candidate selection or imply deployment authority.

The host confirmed that both ARM failures for this candidate returned `68` after notarization was `Accepted`, with CloudKit `NSURLErrorDomain -1005` (connection lost). In the first run, initial staple and validate succeeded before a later separate validate returned `68`; this evidence warrants retries for both exact staple and validate calls, not a wider exit-code policy.

## Options considered

1. **Change candidate packaging source** — simpler retry placement, but creates a different candidate and invalidates the approved immutable identity.
2. **Add generic candidate/target inputs** — reusable, but expands operator and compatibility scope beyond this exact repair.
3. **Use a separate, fixed-identity incident controller** — preserves candidate source and limits extra behavior to one operation and ARM target; requires distinct provenance and workflow tests.

## Decision

Use `prepare-incident-237100-arm64`, with no candidate/target/tag inputs, and a gate over candidate git objects and main ancestry. Keep normal preparation/release matrices and source paths unchanged. Materialize the exact workflow-commit controller blob under `RUNNER_TEMP` only after the clean candidate native build. Scope `XCRUN` to the unchanged candidate packaging command. Delegate non-matching calls by `os.execv` to `/usr/bin/xcrun`, without additional credential-bearing argument logging.

Retry only exact temporary ARM app staple/validate calls. Use four attempts, delays 10/30/60 seconds, a 45-second per-command timeout and 300-second execution/retry budget per invocation. The nominal scheduled maximum is 280 seconds (`4 × 45 + 10 + 30 + 60`); mandatory SIGKILL and child reaping can exceed that budget for a process in uninterruptible kernel sleep, so it is not an absolute wall-clock promise. Add a 30-minute runner timeout only to the protected packaging step, leaving the long native build separate. Retry only the observed code 68 (ticket-service unexpected response / unavailable notarization ticket). macOS `stapler(1)` identifies 65 as invalid ticket data; it remains fail-fast, as do malformed input, revoked ticket, disk write errors and other codes. Suppress stapler child output and emit static messages. Kill and reap child process groups on timeout or cancellation; neither timeout nor failure bypasses any downstream gate.

Keep the candidate's schema-1 preparation receipt intact. Add separate sanitized controller/candidate/operation/target/run/attempt/retry-policy provenance to the Actions preparation bundle. That provenance distinguishes the single-architecture incident from normal `prepare-main`; the packaging receipt alone is not workflow-origin proof.

## Consequences

- **Positive:** Candidate bytes, source provenance and all signing/notarization/Gatekeeper/headless checks remain authoritative. No publication or deployment authority is added.
- **Negative:** This controller is intentionally not reusable for another candidate, architecture, or retry policy. Invalid ticket data fails even if another submission might eventually succeed.
- **Neutral:** Source tests qualify orchestration, not Apple service behavior or real protected artifacts. A successful exact protected run and independent source/run association remain necessary.

## References

- `specs/desktop-release/requirements.md` REQ-DESKTOP-REL-012
- `specs/desktop-release/desktop-release.allium` — `IncidentPreparation`
- `scripts/incident_stapler_controller.py` — `verify_candidate`, `run_stapler`, `wrapper_main`
- Host-confirmed protected ARM failure evidence; macOS `stapler(1)` diagnostics
- ADR-126 and `specs/compatibility/requirements.md` REQ-COMP-015 (unchanged admission)
