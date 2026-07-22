Reduce background request and render churn from panels that are hidden, collapsed, or already refreshing.

Add in-flight guards and visibility/collapse gating to work-scope inventory, file-tree root refresh, local-services discovery, and process-inspector polling. Preserve live behavior while visible, refresh immediately on reopen, retain prior data during refresh, and use bounded retry backoff after failures. Keep each panel's existing endpoint contract. Add fake-timer tests proving no overlap, pause/resume behavior, and stale-response safety.

Frontend request suppression only; do not change process sampling or durable runtime architecture.
