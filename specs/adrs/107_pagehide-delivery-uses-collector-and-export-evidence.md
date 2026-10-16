# ADR-107: Departing-document delivery uses collector acceptance and export evidence

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-CVT-004, REQ-CVT-005, REQ-CVT-006

## Context

The telemetry transport is a best-effort keepalive attempt with one inflight report, a three-second abort deadline and no retry or offline queue. The pagehide qualification driver introduced a departing-page response204 assertion in commit22e73bffa, reinforced by the final-drain checks in3726c1d42. That browser-observer rule was a harness choice, not a requirement to guarantee response visibility after document departure.

Actual candidate failures contain a sampled, correlated incomplete report followed by producer rejection and page/context request failure before cleanup, without a response observation. A separate direct synthetic Chromium149 probe (run38842011920, artifact12021448843) reproduces this pattern in all four latency/departure cells while the synthetic server receives the complete body and finishes204. It does not establish collector receipt or export for any candidate request, nor prove why Chromium reports cancellation. Synchronous dispatch did not eliminate the candidate failure.

The user selected “Collector + export proof (recommended)” with the explicit condition “Owner updates acceptance contract, adds correlated collector/export evidence, and obtains review before changing or rerunning gate.”

## Options considered

1. Retain departing-document browser204 as the delivery oracle: keeps the existing driver assertion, but confuses response visibility in a departing document with server receipt.
2. Require exact-ID real collector acceptance plus actual export: uses the surviving server and trace boundaries while keeping browser-derived admission, report identity and correlations authoritative for expectations.
3. Accept a browser POST body, synthetic response completion or keepalive option alone: insufficient evidence of real candidate acceptance or export; rejected.

## Decision

Use option2 for actual cross-document pagehide only. Preserve nondeparting browser204 checks. Require one real authenticated collector acceptance and one exported journey for each browser-observed sampled pagehide report, matching exact identity and browser-reported request/SSE/trace correlations with independent observed provenance under the candidate run/service. Existing correlation caps and loss counters remain intact; the oracle does not demand uncapped inclusion of every observed request. Collector evidence comes from the validated success path immediately before204, not proxy receipt, request start or export inference. Missing, duplicate, foreign, rejected and invalid evidence fails closed. Existing raw/index export reconciliation and normal app-first receiver-drain ownership remain mandatory.

No producer retry, queue, sendBeacon substitution, sampling change, timeout increase or delivery guarantee follows from this decision. A browser-rejected fetch remains recorded as such even if server acceptance is independently established. Contract review precedes implementation of the changed gate and a candidate rerun.

## Consequences

- **Positive:** delivery qualification separates browser response visibility, collector acceptance and exporter retrieval.
- **Negative:** qualification must capture a bounded, content-free collector witness and join three independently observed inventories; loss of a witness prevents acceptance even if delivery may have happened.
- **Neutral:** synthetic observations inform the evidence contract but never qualify app delivery; prior candidate failures remain failures under their original gates.

## References

- `specs/conversation-view-telemetry/requirements.md`
- `tests/e2e/conversation-view-telemetry-qualification.md`
- `view_journey::report`, `runPendingPagehideCheck`, `retrieve`, `validate_view`, `validate_correlations`, `final_index`
