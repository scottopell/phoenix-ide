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

## Owner bootstrap latency boundary

Normal owner application startup now requests `/api/auth/status`, then `/api/capabilities` after successful authentication. `ApplicationBootstrap` blocks mounting the owner providers/pages until a capability response is decoded. HTTP/network/schema failure presents an explicit error and mounts no owner page; there is no assumed-live fallback. This is a new normal-startup request and failure gate. Public token-share routes bypass owner auth/capability fetching; transition from share to owner re-enters the authentication/bootstrap boundary.

A **direct opening / fresh document** includes asset loading and App mount, authentication, capability round trip/decode, owner mounting, routing, data hydration and rendering. If investigating the original perceived conversation-opening latency from that entry condition, measure the complete navigation-to-ready span and retain the bootstrap request intervals. An already-mounted list-click pilot starts only after the list and owner capabilities are ready: its setup explicitly excludes auth/capabilities and initial mount costs. It cannot measure or explain direct-opening/bootstrap latency, and those costs must not disappear from the original opening-latency question. Browser-cold and database/OS-cold are separate conditions; neither is established by a new navigation alone. Do not manufacture equivalence with subtraction.

## Privacy and fidelity

Capture remains a separately authorized operation. The sanitizer opens the source read-only/query-only in one transaction and reconstructs allowlisted rows rather than copying a raw production database. It excludes credentials, jobs, subscriptions, runtime obligations and migration descriptions. Known heterogeneous content is reconstructed; prose, titles, paths, model identities and arbitrary tool data are synthetic. Unknown fields/columns fail closed. It writes only private `fixture.db` (0400) and `manifest.json` (0600) in a new ignored directory (0700), with FK/count checks.

Schema DDL must be trusted Phoenix-managed, nonsensitive schema: schema definitions are reproduced, not sanitized for arbitrary private literals. IDs, topology, timestamps, lengths and tool mix remain private. Do not publish the fixture or private manual recordings/cookies.

Synthetic prose does not preserve Markdown/code semantics; attachments and opaque display/usage metadata are omitted; states are normalized; runtime load is absent; the database is repacked. The target-only list omits production's large legacy-list response. These differences prohibit production-parity or performance-improvement claims.

## Current qualification and calibration decision packet

Fresh local normal-binary list-click proof: real current-member observation 200; six completed API resources 200; meaningful visible target tail sequence 1499; 1px bottom gap; no input or denied ancillary PR request. Source/binary/loaded assets/fixture hashes are retained in `.phoenix/replay-proof/` (private, host-local). See [dependency/evidence log](conversation-replay-dependency-map.md) for exact validation and unresolved evidence.

The original negative-startup sentinel failure remains disclosed: an unsolicited connection reached a host-global discovery-range listener after preceding fixture/file/log assertions passed. Other local Phoenix instances can scan that range; historical peer attribution is unknown. Diagnostics retain a bounded request prefix on future failures; no connections are filtered or waived. Later isolated/module passes do not explain the original failure.

Historical production proxies (~3.49–3.67s) and fake-replay proxies (~0.98–1.50s) are not comparable benchmark results. Recovered historical trace `88d80ee36460da15cc0e86dd58e2f3ef` placed 2365.945ms in composite message-page work versus 78.543ms aggregate; its request limit and deployed commit association are unverified. No optimization or budget follows from those observations.

Before calibration, decide with the user/Global: (1) required fidelity/comparison boundary and treatment of missing live/list workload, (2) debug-versus-release backend and host/browser identity, (3) cache/reset and concurrent-workload policy, (4) pilot/repetition/warmup/GC/throttle budgets, and (5) acceptable readiness/error criteria. No settings or threshold are assumed here. Resource #888 and standalone UI finalization/link/Close changes require separate composed integration validation; observation must never construct a collector, and ADR numbering collision must be reconciled before combined landing. No held #889 parser changes are ingested.

## Approved local stage diagnostic — 2026-10-11

One approved eight-attempt round completed (initial1, warmups2–3, measured4–8), within35.9s of the five-minute budget. Existing immutable fixture and optimized UI stayed byte-identical; instrumented debug binary hash `9b64132b84ab23fa161f76e7e421615ddd96603b785e5dad8edc6129ad9ebd4a`, source patch over833367925 hash `8ea431ae5324033724a869c7799e18d4f46edcf8d5700ad3e0da87c2a0d5e5ac`. No optimization or SQL/order change. Owner window@8 was rebuilt/relaunched before the measurement budget; browser warm-state setup retained. No production query/deploy, new capture, fixture mutation, forcedGC/cache purge or other-owner shutdown occurred.

Observation startup installs a local timing-only tracing layer: safe UUID correlation plus request kind, numeric limit and cursor/open presence; no payloads, raw query/reference/header fields or events are exported. Shared read spans delineate preparation, combined SQL/fetch+row decode, clone, attachment file/image fetch and hydration, and replacement. Async elapsed includes waits; busy/CPU is not measured. `try_map` decoding remains within fetch to avoid changing semantics. Product target admission and member admission/stable read/metadata are separate. Request elapsed ends at response construction, not body streaming/compression/transfer. Browser resources retain transfer completion and request parameters. Nested spans overlap; no totals or guessed overhead are subtracted. Synchronous timing serialization and stderr writes add observer overhead, especially to parent stages; all spans create extension state, including those not exported.

Raw request-stage/browser/harness records and failures are private under `.phoenix/replay-proof/diagnostic-*`. There are24 correlated detail/member requests, exactly three per attempt. All succeeded; each attempt verified visible meaningful1499, 1px anchor, hydrated member, unavailable live/input and no textarea/denied ancillary/command resource. All8 had zero browser >50ms long tasks; React actual durations, CPU and heap unavailable. Load averages before93.87/94.76/87.99 and after73.38/91.29/87.72 show substantial ambient interference; no paired significance/baseline claim is made.

| Attempt | Role | Visible/frame proxy ms | Limit1 admission / fetch+decode / request ms | Limit100 admission / fetch+decode / request ms | Member admission / stable / request ms |
|---|---|---|---|---|---|
|1|Initial|966.8 /989.0|87.314 /155.461 /288.874|87.772 /395.761 /552.014|46.681 /3.052 /62.484|
|2|Warmup|966.2 /968.4|92.456 /163.502 /302.938|90.132 /391.867 /549.137|48.254 /2.880 /62.942|
|3|Warmup|952.8 /980.9|88.520 /155.747 /288.334|87.926 /396.455 /551.865|48.848 /2.653 /63.341|
|4|Measured|949.9 /960.2|90.930 /154.752 /290.771|87.904 /394.567 /549.009|45.602 /2.731 /60.276|
|5|Measured|958.3 /966.8|89.218 /155.998 /290.771|89.874 /394.172 /555.406|45.670 /2.961 /60.452|
|6|Measured|955.1 /963.5|88.542 /154.702 /286.819|88.731 /400.258 /555.440|49.157 /2.858 /64.447|
|7|Measured|948.5 /980.7|89.847 /150.968 /285.936|89.473 /397.992 /553.741|46.355 /2.480 /60.588|
|8|Measured|958.6 /977.1|89.021 /155.123 /288.376|88.584 /394.516 /550.423|48.497 /3.120 /63.412|

Preparation0.008–0.023ms, clone0.014–0.139ms, attachment parent0.238–0.534ms, replacement0.007–0.040ms across detail requests. File/image fetch and hydration raw spans are retained; member metadata0.130–0.413ms. First response completion occurred319.1–338.7ms from pre-dispatch instrument start; last response947.2–965.3ms. First verified visibility followed last response1.1–1.5ms. These are instrumented elapsed observations, not CPU-exclusive measurements. The small request itself still spends~151–164ms in combined page fetch/decoding, despite returning only one message. This points the next fidelity discriminator at page-query/physical/cache and decoded-payload work, not clone or attachment restoration as the first adjustment. Historical production message_page2365.945ms has unknown limit/source association and lacks these substages; no production causality is inferred.

Qualification: independent bounded four-file static review found no blocking instrumentation defects (before final stderr sink/import-only repair); parent reviewed that repair. Privacy/correlation unit1, shared page tests22, observation tests8, attachment tests3, final positive actual-executable isolation1 and fmt/clippy checks passed. An initial stdout timing sink broke the startup harness after its banner reader closed stdout; fixed to stderr and actual request/isolation regression requalified. A separate negative startup sentinel failure captured local-discovery traffic again; it remains strict and disclosed, not silently rerun to green or used as a performance attempt.

Recommended smallest next fidelity action is **not immediate fixture editing**: separately authorize matched limit1/100 query-plan/index/layout evidence and target-page decoded-payload size distribution. If payload evidence warrants a fixture change, preserve only sanitized target display/usage structure and byte distribution rather than restoring omitted live systems or fabricating delays; if plan/layout dominates, that payload adjustment is not justified. No follow-on round is executed. Owner bootstrap remains excluded by list-ready setup; direct-opening latency is a separate measured boundary.
