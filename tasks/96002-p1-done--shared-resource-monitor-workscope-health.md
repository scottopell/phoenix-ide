# Shared demand-driven resource monitor and Work Scope health

## Outcome

Replace independent `/about` and process-inspector resource sampling with one demand-driven server monitor, then use the shared observations to add live CPU, proportional-memory, and process-count health to Work Scope rows and summaries.

Keep bash output tailing separate: task 96001's output-only ring reads are not resource telemetry and must not wait on or trigger process sampling.

## Required behavior

### Shared monitor

- Activate sampling only while at least one `/about`, Work Scope health, or process-inspector consumer is subscribed/actively requesting fresh data; stop after the last consumer leaves, with a small bounded lease if needed to prevent churn.
- Discover the authoritative union of attributable Phoenix-managed process identities once per generation, sample CPU and proportional memory once, and preserve PID-start identity checks and capability-gap logging.
- Produce one timestamped typed observation generation from which global managed totals, category rows, Work Scope aggregates, and bash-handle aggregates are derived without resampling the OS.
- Deduplicate native PIDs in global and scope totals while retaining typed ownership membership for projections.
- Ensure concurrent requests share an in-flight generation rather than starting duplicate two-sample CPU intervals.
- Expose freshness explicitly (`sampled_at` and stale/unavailable semantics); never encode unavailable metrics as zero.
- Keep snapshots ephemeral and bounded; no database persistence or unbounded history.
- Handle process start/exit, PID reuse, registry changes, consumer disappearance, slow sampling, and shutdown without leaked tasks or stale ownership.

A request-coalesced lease/cache is acceptable if it satisfies the demand-driven behavior more simply than a long-lived subscription protocol. Do not introduce SSE unless it is justified by the final API/UI contract and all cross-spec obligations are updated.

### Existing consumers

- Migrate `/api/about/resources` to derive its host, managed-total, category, and process rows from the shared observation generation without changing its user-visible metric definitions.
- Migrate live process-inspector resource metrics to the same generation, while preserving its independent incremental bash-output cursor and terminal behavior.
- When `/about` and one or more inspectors are active concurrently, verify that one sampling generation serves all consumers and that their overlapping values/timestamps agree.
- Preserve fast, honest behavior when a fresh generation cannot be produced: last-good/stale where the owning surface supports it, typed unavailable otherwise.

### Work Scope health UI

Extend the Work Scope observability projection and persistent grounding-panel section with resource health for attributable live resources:

- Per live bash handle: CPU percentage, proportional memory bytes, and process count from the shared observation generation.
- Scope summary: deduplicated CPU, proportional memory, and process count across attributable resources in that scope.
- Expanded rows remain dense: lifecycle glyph, label, elapsed time, then compact health values; detailed command/output/identity stays behind disclosure and `inspect →`.
- Collapsed rail retains the live count and adds only a restrained attention marker when a typed health condition warrants attention; do not turn the rail into a dashboard.
- Define attention thresholds and hysteresis centrally and test them. Prefer descriptive `high CPU`, `high memory`, or `process spike` states over a vague “runaway” label. Avoid alerting where metrics are unavailable or stale.
- Distinguish freshness/capability gaps from true zero usage.
- Preserve the chain dock's data-source constraints and shared row vocabulary; if live health cannot be supplied safely on that surface, show lifecycle data without fabricating metrics and document the structural reason.
- Resource observation updates must not churn the transcript or automatically open the inspector.

### Correctness and specifications

- Update `specs/deployment-info`, `specs/process-inspector`, and `specs/work-scope-ui` requirements/executive documents to describe the shared demand-driven source, projections, freshness, and UI behavior.
- Update or add an ADR for the central observation-generation decision and alternatives; do not put rollout history in timeless requirements.
- Respect the existing rule that the Work Scope inventory is an authoritative read projection, with no parallel semantic representation of resource ownership.
- Use typed Rust wire types and generated TypeScript; do not hand-edit generated files.
- Normalize ownership/membership where it is durable or independently queried; ephemeral whole-generation observations may remain an in-memory typed aggregate.
- Run the spec-authoring pre-flight checklist and Allium validation for every touched behavioral spec.

## Verification

Add focused backend tests for:

- concurrent consumer request coalescing and one sampling pass per generation;
- activation/idle teardown, no leaked background sampler, and shutdown;
- PID deduplication across global/scope/handle projections;
- PID reuse and exited-during-sample handling;
- process-group membership changes between generations;
- unavailable metrics and debug-visible capability gaps;
- `/about` and inspector parity from one generation;
- Work Scope aggregate and per-handle projection correctness;
- bounded cache/lease and freshness semantics.

Add focused UI tests for:

- live Work Scope rows showing CPU, memory, and process count;
- unavailable/stale values never appearing as zero;
- scope summary and restrained collapsed attention state;
- threshold hysteresis and recovery;
- lifecycle-only fallback where health data is unavailable;
- polling/subscription start, coalescing assumptions, stop on hidden/unmount, and stale-response guards;
- `/about` and inspector regressions.

Run codegen, focused Rust and UI suites, `./dev.py check`, and real-browser QA of the populated grounding-panel fixture plus `/about` and inspector concurrency. Capture evidence that concurrent surfaces no longer multiply OS sampling generations.

## Delivery

- Commit logical units as they settle.
- Push the branch and open a PR with a concise concept-focused description and validation evidence.
- Wait synchronously for the automatically triggered Codex review by using a bash handle and repeated bounded `wait` calls; do not busy-poll.
- Address actionable findings, revalidate, commit, and push to retrigger review. Repeat until approval when productive.
- Abort the review loop and report clearly if it becomes wasteful: repetitive findings on settled decisions, service errors, or unexplained absence after reasonable bounded waits.

## Acceptance criteria

- `/about`, open process inspectors, and Work Scope health consume one demand-driven observation generation rather than independently sampling overlapping PIDs.
- No consumers means no recurring process sampling.
- The Work Scope grounding section identifies which live bash handle is consuming CPU, proportional memory, or process count without requiring inspector drill-in.
- Expanded and collapsed presentations remain information-dense and non-noisy, with typed freshness and attention behavior.
- Existing output tail, lifecycle, `/about`, and inspector contracts remain correct.
- Full validation passes, the branch is pushed, a PR is open, and the Codex review loop ends in approval or a clearly justified abort.
