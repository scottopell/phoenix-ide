# Coordinator tracking status

Manual storage, native tools and durable context integration are implemented; qualification remains in progress. Requirements define durable memory independently of Goals, subscriptions and worker status. Scanner activation is a later gate after manual request/bookmark → detour → continuation → return is demonstrated.

| Requirement | Status | Verification |
|---|---|---|
| REQ-TRACK-001–005 | Deterministic checks pass; live qualification pending | Twelve schema/service tests and one actual native registry/service test pass, including source ordering, source gaps, retirement history and encoded output bounds |
| REQ-TRACK-006–007 | Qualification in progress | Prefix/DB continuation, causal sequence admission and ownership-loss tests pass. The complete tool loop passed legacy input; strengthening it to actual accepted/claimed UserApi admission exposed a fixture-adoption blocker under investigation |

Initial bounds: eight compact index entries; full notes only in detail reads; eight evidence/retirement entries per detail page with explicit remaining cursors; complete detail at most 96 KiB; combined editable metadata at most 16 KiB serialized JSON and complete evidence entries/retirement reasons at most 4 KiB serialized; compact observation at most 8 KiB; 24-hour explicit-review/surfacing cooldown with new-evidence exception. A persisted per-input observation identity prevents repeated cues on retry; per-item attention receipts survive continuation and never advance explicit review. These are initial bounds, not measured provider savings.

No production deployment or real-model acceptance is claimed. The isolated Astra/high trial recovered both saved stopping points across a real continuation and kept neutral arithmetic turns quiet. Its due-item check lacked a host cue because legacy test inputs were marked generated, so it does not qualify attention. Provider calls remain gated on the corrected authoritative-input deterministic fixture.

The focused behavioral layer is `coordinator-tracking.allium`; its structural checker passes. The executable gates include normalized retirement pointers, paginated reason history, exact recording provenance, sequence reservation, worst-case escaped context bounds and historical-cue expiry. The opt-in real-model gate remains pending; deterministic test passes do not establish useful model follow-through. Live tests use a disposable database/home, actual registered tools with effectful operations denied, explicit model selection and hard request/output/time budgets. Neutral arithmetic turns test default silence, and an unrelated greeting tests due-request recovery after continuation.
