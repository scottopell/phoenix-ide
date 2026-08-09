# Consolidate TaskApprovalReader find corpus

Reduce repeated Markdown parse/projection payload in the shared-find integration tests while preserving prose, same-line table-cell, code, close/clear, focus, and navigation contracts. Capture repeated before/after Vitest CPU and wall measurements, deliberate fault proof, focused/full validation, and a separate PR.

## Result: rejected

Three shapes were measured and restored: a combined navigation/clear/table journey regressed to 347.683 ms median CPU versus 262.851 ms summed per-test baseline; a navigation+clear journey improved whole-file CPU only 1.4%, within noise; and merging clear/close into the existing overlapping-text journey measured 3,715 ms candidate versus 3,710 ms baseline CPU in 10 interleaved A/B pairs, with wall medians 2,610 versus 2,605 ms. The per-test worker windows also shifted cost into untouched following tests when test order changed, so they are not reliable evidence for this consolidation. No production or test behavior change is justified. Raw evidence is under `target/qa-evidence/53009/`.
