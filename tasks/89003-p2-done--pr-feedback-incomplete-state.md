# Surface a distinct "feedback incomplete" state when PR feedback fetch degrades

The auto-fix freshness signal (`PrFeedbackFreshness`, New/Edited) now represents
only genuine content change. Coverage gaps and fetch failures were previously
folded into a generic "updated" label, which conflated a real reviewer edit with
a transient gh error. That conflation was removed: `feedback_freshness_from_baseline`
now returns no freshness signal when feedback cannot be fetched or a surface is
unavailable (see `crates/phoenix-ide/src/api/pr_monitoring.rs` and the Err branch
in `attach_pr_feedback_freshness`, git_handlers.rs).

That means a degraded fetch is currently invisible to the user — they see no badge
at all, even though Phoenix could not confirm whether new feedback exists.

Build a separate, explicit error/incomplete state (distinct from New/Edited content
freshness) so the UI can show something like "feedback may be incomplete" with the
failing surface(s). It must be visually and structurally distinct from a content-change
signal — do not reuse the New/Edited variants. Coverage status per surface is already
tracked (`PrFeedbackCoverage` / `PrFeedbackCoverageStatus`).
