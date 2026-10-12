# Capability-composed conversation replay

This local development loop uses the **normal Phoenix binary, router and UI** with persisted observations and independently rejecting command admission. It is not a production clone, simulation, optimization result or calibrated baseline. Architectural contract: [REQ-API-030](../specs/api/requirements.md) and [ADR-092](../specs/adrs/092_observation-and-execution-are-separate-application-capabilities.md).

## Existing local fixture

```sh
./dev.py conversation-load serve \
  --fixture .phoenix/conversation-load-fixture-v3 --port 18043

# Configuration only; does not execute a sample.
./dev.py conversation-load scenario \
  --fixture .phoenix/conversation-load-fixture-v3 \
  --url http://127.0.0.1:18043/ > .phoenix/conversation-load-scenario.json
```

The retained owner uses tmux window `@8`; stop/restart only that owner before rebuilding. Port 8044 has unknown ownership and must not be killed. `serve` builds optimized Vite assets and the debug **phoenix_ide** binary; `--no-build` requires already matching assets/binary. A release backend is a separate explicitly identified comparison. Do not replace an immutable database underneath a running listener.

No additional capture, benchmark repetitions, cache reset, throttling or calibration budget is authorized by these commands. The generated configuration retains the original exploratory one-run settings (CPU rate 1, no discarded warmups, no forced GC); those are not accepted repeated-benchmark settings. Do not execute even an exploratory sample without the applicable scope admission.

## Authority and read path

Composition is selected before live startup. Observation constructs no AppState, runtime, collector, provider/tool registry, recovery/continuation workers, discovery or worktree reconciliation. The database must be standalone, read-only, regular, without hardlinks or SQLite sidecars; immutable opening does not migrate. Observation access requires a loopback peer and rejects forwarding headers. Remote hosting is not supported by this access contract.

One production router admits shared persisted reads and rejects effectful handlers through `LiveState` before body extraction or transport upgrades. `/api/capabilities` explicitly reports supported observations and absent live/input/provider/host capabilities. It remains owner-authenticated in the normal live composition. Public `/s/:token` uses its existing token-authorized stream and does not bootstrap owner capabilities.

The target product list uses the existing real database/list projection and filters to the admitted target. Product detail uses shared aggregate/topology/message-page reads. `/api/conversations/:id/observation` uses persisted member metadata and aligned bounded transcript reads, reporting persisted provenance and unavailable live process facts. The aggregate page mounts that current-member owner without a composer, EventSource or second transcript renderer. It does not fabricate a legacy live conversation, static SSE, provider validity or successful command.

The obsolete `load_benchmark` module/binary and auxiliary-response generator have been removed. Their two router regressions (read/pagination/immutability and denied-runtime/write paths) are superseded by normal-router observation, application-state admission, immutable-fixture and actual-executable startup tests. Auxiliary-cap/static-init Python tests are replaced with no-aux/current-member manifest and truthful scenario-readiness regressions. Existing ignored `aux.json` is historical fixture material, not loaded or regenerated.

## Truthful readiness and evidence

The scenario uses the existing `browser_profile.run_scenario` harness. Setup waits for the real target list row outside the timed window. The list click must reach the canonical target route, hydrate its persisted current member, show explicit unavailable live connection/input, contain **no textarea**, and render the exact expected tail with meaningful visible content in a nonzero-height transcript. Readiness also requires a bottom gap within rounding tolerance. The two animation-frame callbacks are a **paint opportunity proxy**, not actual paint measurement or timed scroll-responsiveness proof. A readiness timeout is failure, not a sample.

After an admitted run, retain raw tool samples/methodology warnings and `window.__loadBenchmarkResult` privately. Record:
- immutable fixture hash, counts, sanitizer omissions and target membership;
- exact Git/source identity including untracked sources, binary hash/profile, loaded asset hashes, browser/viewport/host;
- route, expected message ID/sequence, member hydration, unavailable input and bottom geometry;
- end-to-end wall latency, actual admission/helper intervals, request statuses/payload sizes and long tasks;
- setup/cache state, concurrent workload and observer overhead.

Do not subtract guessed admission overhead. Admission pre-reads themselves warm SQLite/OS caches; subtracting measured wall time does not establish cold/live equivalence. Keep end-to-end timing and comparable stage boundaries. Separate real scroll interaction from readiness proxy measurement.

## Privacy and fidelity

Capture remains a separately authorized operation. The sanitizer opens the source read-only/query-only in one transaction and reconstructs allowlisted rows rather than copying a raw production database. It excludes credentials, jobs, subscriptions, runtime obligations and migration descriptions. Known heterogeneous content is reconstructed; prose, titles, paths, model identities and arbitrary tool data are synthetic. Unknown fields/columns fail closed. It writes only private `fixture.db` (0400) and `manifest.json` (0600) in a new ignored directory (0700), with FK/count checks.

Schema DDL must be trusted Phoenix-managed, nonsensitive schema: schema definitions are reproduced, not sanitized for arbitrary private literals. IDs, topology, timestamps, lengths and tool mix remain private. Do not publish the fixture or private manual recordings/cookies.

Synthetic prose does not preserve Markdown/code semantics; attachments and opaque display/usage metadata are omitted; states are normalized; runtime load is absent; the database is repacked. The target-only list omits production's large legacy-list response. These differences prohibit production-parity or performance-improvement claims.

## Current qualification and calibration decision packet

Fresh local normal-binary list-click proof: real current-member observation 200; six completed API resources 200; meaningful visible target tail sequence 1499; 1px bottom gap; no input or denied ancillary PR request. Source/binary/loaded assets/fixture hashes are retained in `.phoenix/replay-proof/` (private, host-local). See [dependency/evidence log](conversation-replay-dependency-map.md) for exact validation and unresolved evidence.

The original negative-startup sentinel failure remains disclosed: an unsolicited connection reached a host-global discovery-range listener after preceding fixture/file/log assertions passed. Other local Phoenix instances can scan that range; historical peer attribution is unknown. Diagnostics retain a bounded request prefix on future failures; no connections are filtered or waived. Later isolated/module passes do not explain the original failure.

Historical production proxies (~3.49–3.67s) and fake-replay proxies (~0.98–1.50s) are not comparable benchmark results. A bounded production trace placed ~2.365s in message-page hydration versus ~60ms aggregate hydration. No optimization or budget follows from those observations.

Before calibration, decide with the user/Global: (1) required fidelity/comparison boundary and treatment of missing live/list workload, (2) debug-versus-release backend and host/browser identity, (3) cache/reset and concurrent-workload policy, (4) pilot/repetition/warmup/GC/throttle budgets, and (5) acceptable readiness/error criteria. No settings or threshold are assumed here. Resource #888 and standalone UI finalization/link/Close changes require separate composed integration validation; observation must never construct a collector, and ADR numbering collision must be reconciled before combined landing. No held #889 parser changes are ingested.
