# Scoped conversation-search query-plan experiment

User authorized one minimal experiment: favor FTS-first scoped retrieval, preserve exact output/provenance and scope-before-LIMIT. Use existing private immutable fixture and frozen scenarios, corrected baseline then paired after runs across all cases. No schema/index changes, production mutation/deployment or platform expansion. Stop/reject if value fails. Keep benchmark PR837 separate; use stacked perf/scoped-conversation-search branch after baseline qualification.

## Adversarial qualification outcome

Measured one-case gain remains ~59x with exact output, but late review r4174511607 reports a credible tiny-scope/common-term counterexample (locator-first ~0.23ms versus forced FTS-first~30ms on300k synthetic corpus). Universal unary+ policy is not qualified; PR838 draft and task blocked rather than invent an unmeasured threshold. Historical before/after evidence preserved. Next bounded action must validate this counterexample and choose/reject a scope strategy; no live change or merge/deploy.

## Counterexample adjudication

Using cached Rust-linked SQLite3.51.3 and production-shaped locator/source/conversation joins, hidden predicate, BM25/order/LIMIT/count/snippet,300kcommon rows15scope, native locator-first~77ms versusFTS-first~24ms; ANALYZE~78ms versus24ms. Exact claimed0.23→30ms regression not reproduced. Specific unsupported review claim dismissed with artifacts under private adjudication/; no universalSLO promise/arbitrarythreshold. Original actualpair source063a/42740 remains historical; current precomputed-oracle setup unmeasured. Ready for exacthead review/CI only, no merge/deploy.

## Current main CI identity refresh

Independent source0ab0a1d85 is unchanged. GitHub regenerated the synthetic merge from cc9df28e3 (tested successfully) to803fae0cce6d7d8807dc3653a4d64f4089ac34bd against unchanged mainabfdcf1; the new merge had no required check associations. This task-only update requests normal CI on the current main merge identity; no source change, new benchmark run, merge or deployment.
