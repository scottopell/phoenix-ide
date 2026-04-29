---
created: 2026-04-29
priority: p1
status: ready
artifact: src/runtime.rs
---

Implement chain-scoped SSE broadcaster registry. Mirror existing conversation-runtime registry pattern in src/runtime.rs. Keyed by root_conv_id. Lazy creation on first Q&A submission; teardown when subscriber count reaches zero AND no in-flight stream. In-flight streams pin the broadcaster alive past zero subscribers until terminal status (completed/failed). Token events carry chain_qa.id discriminator so multi-tab subscribers can demultiplex concurrent Q&As. Add ChainTokenEvent SSE wire type with ts_rs derive for codegen. Tests: lifecycle (create/teardown), in-flight retention, multi-subscriber demux.
