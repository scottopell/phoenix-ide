# Align persisted WorkScope IDs with the typed parser

`WorkScopeId::parse` rejects trim-empty values, but the existing `work_scopes.id` schema and some dependent columns enforce only non-empty or TEXT storage. Direct/manual malformed rows can therefore persist whitespace-only IDs and later panic or produce serialization failures in typed read paths.

Audit the full persisted WorkScope ID schema family, add a migration that makes trim-empty identities structurally unrepresentable without rewriting opaque valid bytes, and add upgrade/read regressions. This is broader than migration 65's dormant GitRepository attachment seam and must not be folded into Foundation/Cutover work without separate review.
