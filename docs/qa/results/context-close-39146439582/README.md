# Hosted context-close observation — bounded negative result

The exact sanitized `result.json` from artifact **12142589938**, job
**117731123144**, run **39146439582**, is retained byte-for-byte. Its checksum,
source identities and runtime versions are in `provenance.json`. Four cases
passed the probe's structural and functional checks without primary or cleanup
errors. This is **not historical causal closure**.

| Schedule | Observed facts |
|---|---|
| Before close | Exact-request context and page listeners observed 204 before close resolved. A later matching `requestfailed` event also occurred; that event does not mean the earlier 204 was unobserved. |
| During close | Release continuation occurred while the close promise was pending. Both listeners observed 204 before resolution. Matching `requestfailed` followed; no internal browser-close milestone is inferred from invocation alone. |
| After close | Server response closed before context close resolved. Release/end was attempted afterward, with `writeAvailable: false`; no end callback, finish or client response was observed through owned browser closure. |
| Never release | No release or client response; held socket disposal and owned browser closure completed. |

No response after successful context-close resolution was reproduced. This
observation does **not support applying** the conditional modeled lifecycle
close-drain proposal. It is one set of explicit schedules on the recorded runtime,
not proof that all possible schedules are impossible.

The probe uses bare fetch with `keepalive: true`, not the actual reporter,
AbortController, pagehide logic, Phoenix collector or export pipeline. The older
stock failure remains unexplained: lost primary/cleanup-only error evidence
cannot be reconstructed by this result. No Stage 2, second probe, natural-sampling
retry, cohort pooling, performance clearance, or live patch is implied.

Reproduction tooling: `ui/scripts/context-close-probe.cli.mjs`; scope, safety and
hosted-only admission: `docs/qa/context-close-probe.md`. This result preserves only
synthetic UUIDs, fixed states, finite event timings and safe runtime provenance;
no fixture database, credentials, URLs, free-form errors or raw OTLP are included.
