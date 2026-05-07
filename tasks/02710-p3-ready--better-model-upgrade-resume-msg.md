---
created: 2026-05-07
priority: p3
status: ready
artifact: src/runtime.rs
---

When a conversation auto-resumes after model upgrade (evict_runtime triggers recovery), the injected system message says "interrupted by a server restart" which is misleading. Should detect model-upgrade context and say "model was upgraded" instead. Requires passing eviction reason through to the recovery path.
