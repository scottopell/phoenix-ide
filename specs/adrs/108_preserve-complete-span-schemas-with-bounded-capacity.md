# ADR-108: Preserve complete span schemas with bounded provider capacity

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-CVT-005, REQ-CVT-006, REQ-CVT-007

## Context

The observability hardening in commit4a1f6bfed7957ddbf35073e202b8324b72d9f199 (PR519, task94005) introduced a32-attribute SDK backstop for both OTLP and Datadog providers. Its explicit rationale distinguishes privacy—the spans-only target/name allowlist and content-free bounded attributes—from defensive count limits; it rejects trusting truncation or raising collector ingestion limits.

The complete collector view schema declares44 semantic fields (1030 attribute-key bytes), including readiness marks after the first32 fields. Pinned tracing-opentelemetry0.32.1 defaults add at most8 location/thread/target/inactivity fields. Required capacity is exactly52. Actual qualification exported ready views without readiness marks, while producer reports contained them. Source establishes that the production32-field cap cannot represent this schema; the real-handler exporter regression exercises the old cap and the production capacity rather than the SDK's larger default. Hosted test execution remains verification evidence, not implied by this decision.

## Options considered

1. Increase the common provider backstop to52: the smallest capacity that represents the existing complete44+8 schema, sharing one limit function across both exporters.
2. Choose64: extra unearned headroom beyond the enumerated schema; rejected.
3. Disable SDK metadata: still leaves44>32 and changes general observability policy without solving capacity.
4. Reorder or drop fields: hides data loss elsewhere and violates the complete report/trace contract; rejected.
5. Split providers or decompose the view into more spans: introduces provider lifecycle or wire/parentage/join changes disproportionate to preserving an existing bounded schema.

## Decision

Use a52-attribute backstop through `phoenix_span_limits()` for both OTLP and Datadog. Retain zero span events, zero event attributes, four links, four attributes per link, existing queue/batch limits, spans-only export allowlisting, content/privacy validation and receiver limits. All44 semantic fields and values must survive actual collector/default-layer export with no dropped attributes; SDK metadata remains a separately typed receiver sink.

This is an explicit global provider-capacity change, not per-view isolation: any allowlisted span may retain up to20 more attributes than before. The declaration inventory finds view44, LLM request29 and direct_turn.settle14 ordinary declared fields plus the special otel.status_code control field, and other direct emitters at most11 semantic fields; DB read spans without explicit target use their module target and at most one declared field. LLM request29+8 can also exceed32, so retaining additional safe LLM attributes is a real global effect, not zero-impact. The settlement helper includes error.message and probe.error.message populated on real error paths; these are not asserted to have a fixed string-byte bound. This capacity decision does not establish their privacy or maximum serialized size. Deferred `record` calls populate declared fields rather than adding arbitrary keys. No larger dynamic application-key export path is admitted.

## Consequences

- **Positive:** the complete existing view schema fits without changing its wire shape, dropping fields or introducing another provider lifecycle.
- **Negative:** maximum attribute-slot capacity rises by20/32=62.5%. At the default queue capacity2048 this is40,960 additional potential slots; it is not a memory/byte bound, measured traffic increase or overhead clearance. Shorter spans do not automatically acquire20 attributes. Metadata string lengths and allocator overhead prevent converting slots into a fixed RAM claim.
- **Neutral:** actual SpanData-to-OTLP protobuf measurements from the bounded populated-report regression report old32 versus production52 span bytes and semantic/SDK key-value bytes; these are fixture measurements, not a universal maximum payload or performance acceptance. Existing receiver budgets remain unchanged.

## Verification

The real collector regression uses complete optional fields, maximum allowed frontend commit length, valid saturated counters at10000 with counters_saturated=true and one typed request correlation; compares old32 loss with production52; asserts44 exact typed semantic values, at most8 known SDK fields, zero drops/events and correlation-child parentage; and measures actual protobuf encoded sizes. The default-layer metadata guard and spans-only/privacy/limit guards remain separate. Both exporter constructors must use the common production function. Compile/run and exact hosted receipts remain required before qualification claims.

## References

- `specs/conversation-view-telemetry/requirements.md`
- `tests/e2e/conversation-view-telemetry-qualification.md`
- `view_journey::report`, `production_attribute_capacity_preserves_complete_ready_collector_schema`, `phoenix_span_limits`
