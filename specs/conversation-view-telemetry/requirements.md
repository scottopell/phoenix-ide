# Conversation-view journey telemetry

## User need

As the Phoenix operator opening Global and ordinary conversations, I need retrievable, content-free evidence separating transcript presentation from breadcrumb usability, so I can distinguish request latency from view-readiness latency without mistaking a commit estimate for actual pixel presentation.

## Requirements

### REQ-CVT-001: Journey identity and lifecycle

WHEN a conversation view is admitted by navigation, reload, Back/Forward, or standalone-PWA resume
THE SYSTEM SHALL assign a fresh opaque journey/open UUID before observing its outcome
AND SHALL sample at admission, independently of success, using a default probability of one quarter
AND SHALL distinguish ready, failed, canceled, and incomplete terminal outcomes
AND SHALL retain missing observations as null rather than fabricate successful timings.

A route replacement cancels the preceding journey. A failed initial request terminates it as failed. A five-minute observation deadline or page suspension ends an unfinished journey as incomplete. PWA foreground resume starts a fresh journey; it does not continue the preceding timing origin. An empty transcript is meaningful only when the authoritative view explicitly presents its empty state, not when a loading shell is empty.

### REQ-CVT-002: Truthful presentation timing

WHEN the view observes shell, snapshot receipt, hydration readiness, meaningful transcript, chapter-data readiness, breadcrumb presentation, breadcrumb enablement, or anchor settlement
THE SYSTEM SHALL retain the first observation on a client monotonic timeline
AND SHALL identify presentation evidence as `commit_frame_estimate`, a commit/frame-scheduling and visible-geometry estimate rather than compositor/pixel proof
AND SHALL derive breadcrumb-ready-minus-meaningful-transcript from the later of breadcrumb presentation and enablement
AND SHALL preserve signed differences when observations arrive out of order.

WHEN a duration exceeds five minutes
THE SYSTEM SHALL explicitly mark the timeline saturated, treat its bounded values as lower-bound/censored observations, and omit the primary readiness difference
AND SHALL NOT silently present clamped values as exact timings.

Client and server wall clocks SHALL NOT be subtracted. Anchor settlement SHALL come from the owning positioning lifecycle and visible rendered target evidence, not an arbitrary elapsed timer, data availability, navigation inventory, or an empty virtual slot. A target positioning result of `applied` SHALL NOT establish presentation while that target lacks visible rendered content. A resume SHALL obtain fresh anchor observations rather than copy a preceding journey's settlement mark. A retained target observation SHALL match the current owning command's navigation/request identity, source/view/transcript generation and selected-segment scope captured at issuance, and SHALL retain missing or superseded disposition without a settlement mark. A retained tail observation SHALL require current owning view state and positive measurable viewport geometry. Telemetry probes SHALL NOT reissue positioning or mutate scroll, visibility, virtualization caches or ranges. Ready requires meaningful transcript, breadcrumb presentation and enablement, and anchor settlement; absent marks remain visible even on a ready report. First-observation marks SHALL remain historical observations, not current visibility predicates. At readiness completion THE SYSTEM SHALL recheck foreground visibility, currently rendered transcript, currently visible actionable breadcrumb and current anchor evidence; an earlier visible or enabled state SHALL NOT qualify content or controls that are hidden, disabled or superseded.

### REQ-CVT-003: Bounded correlation and privacy

WHEN a sampled journey is reported
THE SYSTEM SHALL allow only fixed typed fields for opaque open/reporter IDs, stable conversation and segment UUIDs, frontend/backend source identities, route/trigger, coarse platform, standalone status, visibility/background duration, fixed readiness marks and outcomes, bounded counters, and request/SSE/trace correlation IDs
AND SHALL bound correlations to sixteen UUID or trace-ID entries and report excess drops
AND SHALL reject unknown report fields and bodies exceeding eight KiB
AND SHALL NOT send message content, breadcrumb labels, raw query strings, filesystem paths, credentials, screenshots, caller-defined attributes, or raw user-agent strings.

Request completion order SHALL NOT be treated as request identity. The open ID links browser evidence to existing detail-request server stages. Correlations represent observed identity only, never guessed server causality. Source identities SHALL identify the independently built frontend and backend; unavailable frontend identity is null rather than inferred from the responding backend.

### REQ-CVT-004: Bounded transport and denominators

WHEN a journey terminates
THE SYSTEM SHALL attempt a best-effort, non-render-blocking keepalive report through the existing telemetry pipeline
AND SHALL permit only one in-flight report per reporter, with a three-second abort deadline
AND SHALL drop a report arriving during that attempt rather than queue or retry it
AND SHALL NOT create a durable offline queue, unbounded retry, per-frame network traffic, or per-token network traffic.

Admission, sampled, transport-drop, and transport-failure counters are cumulative within an opaque per-reporter lifetime. Consumers SHALL use the latest observed counters per reporter, not sum cumulative counters across reports. Counters cap at ten thousand with explicit saturation; a cap is a lower bound, not an exact denominator. Reports lost before export cannot be reconstructed; observed reports are not a complete census. Existing collector sampling, retention, and exporter-drop policy apply without production configuration changes. Telemetry does not become a transcript/session recorder or a source of conversation truth.

### REQ-CVT-005: Qualification

THE SYSTEM SHALL expose deterministic tests for injected request versus view delay, missing/reordered marks, cancellation, background/resume and duration saturation, privacy projection, correlation bounds, sampling, and transport failure/drop behavior
AND SHALL qualify actual candidate Global and ordinary journeys with a retrievable correlated trace before claiming real-server acceptance.

Synthetic timing controls SHALL be distinguished from candidate-browser/server evidence. Telemetry SHALL NOT claim to reconstruct an incident that occurred before instrumentation existed. Attribution stages SHALL be added only where evidence warrants them, not as a general profiling framework.

### REQ-CVT-006: Lifecycle-specific delivery evidence

WHEN qualification observes a sampled report from an actual departing document
THE SYSTEM SHALL require independently captured acceptance by the real authenticated collector and actual exported journey evidence for the exact producer-admitted journey identity
AND SHALL join collector acceptance and exported trace by the browser-admitted journey identity and authoritative backend source/server instance
AND SHALL match exported frontend source, route, trigger, terminal outcome and observed request correlations against the independent browser inventory
AND SHALL reject missing, duplicate, foreign, rejected or privacy-invalid evidence.

A departing document's rejected fetch or missing response observation SHALL NOT substitute for collector rejection or acceptance. Collector acceptance SHALL be observed only after authentication, bounded decoding, semantic validation and authoritative server identity resolution succeed. Collector acceptance alone SHALL NOT prove export, and export-derived expected inventories SHALL NOT manufacture browser observations. Synthetic-server response completion SHALL NOT qualify a real candidate. For nondeparting documents, qualification SHALL retain the browser-observed collector204 requirement in addition to exported evidence. An unsampled admission SHALL remain a sampling observation, not a successful delivery case. These evidence rules SHALL NOT add retries, a queue, a delivery guarantee, altered sampling or a longer transport deadline.
