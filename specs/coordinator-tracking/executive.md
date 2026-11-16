# Coordinator tracking status

Manual storage, native tools and durable context integration are implemented; qualification remains in progress. Requirements define durable memory independently of Goals, subscriptions and worker status. Scanner activation is a later gate after manual request/bookmark → detour → continuation → return is demonstrated.

| Requirement | Status | Verification |
|---|---|---|
| REQ-TRACK-001–005 | Qualification pending | Initial five schema/service tests pass; native registry/service and extended history cases implemented |
| REQ-TRACK-006–007 | Qualification pending | Production registry/prompt constructor, prefix, actual DB continuation and quiet-attention tests implemented; all-target compile passes |

Initial bounds: eight compact index entries; full notes only in detail reads; 16 evidence/retirement entries per detail page with explicit remaining cursors; compact observation payload at most 8 KiB; 24-hour explicit-review/surfacing cooldown with new-evidence exception. A persisted per-input observation identity prevents repeated cues on retry; per-item attention receipts survive continuation and never advance explicit review. These are initial bounds, not measured provider savings.

No production deployment or real-model acceptance is claimed.
