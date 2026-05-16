Add a browser_profile tool (single tool, action enum) bringing systematic web
performance testing into Phoenix, ported/expanded from Shelley's
claudetool/browse/profile.go and scoped to the lading-style tiered
requirements (reproducible scenario + baseline + significance + variance).

Tier 0 (must-have): deterministic scenario driver + N-run harness returning
RAW per-run samples (hard constraint: never pre-averaged), CPU throttling,
Performance.getMetrics macro counters, React commit metrics, forced-GC heap.
Tier 1: CPU sampling profile, why-did-render, timeline trace + long-task
extraction (>50ms), heap-snapshot diff. Cheap Tier 2: JS coverage,
trace-to-disk. Deferred: network emulation (2.1).

Spec: specs/browser-tool/ REQ-BT-019 + browser-profiling.allium lifecycle.
React commit collection extends the existing __phoenix helper (react.rs).
