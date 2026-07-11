# Investigate Codex-auth prompt caching from source to live wire

## Problem

Phoenix deliberately gates GPT-5.6 `prompt_cache_options` and explicit `prompt_cache_breakpoint` fields away from the ChatGPT/Codex-auth backend. That is the safe behavior for PR #475, but it is based on an incomplete contract: public OpenAI Responses documentation describes the direct platform API, while the current upstream `openai/codex` client sends only a stable `prompt_cache_key` to the ChatGPT backend, including for GPT-5.6 Sol/Terra/Luna.

Upstream omission is evidence, not a complete answer. We have not established whether:

- the ChatGPT Codex backend rejects, ignores, strips, or honors GPT-5.6 cache options and explicit breakpoints;
- Responses Lite or WebSocket transport changes caching semantics;
- the backend reports `cache_write_tokens` even though upstream codex-rs currently parses only `cached_tokens`;
- remote compaction, local history shaping, model switches, tool-definition changes, or reasoning/config overrides preserve the exact cacheable prefix in practice;
- Phoenix's request shape and stale-tool-result clearing achieve comparable cache hit rates to upstream Codex under Codex authentication.

Without runtime evidence, removing the gate risks invalid requests, while keeping it indefinitely may leave substantial latency and token-cost savings unused for users authenticating through Codex.

## Objective

Build an evidence-backed model of prompt caching on the ChatGPT/Codex-auth path, using upstream codex-rs as the primary implementation source of truth and controlled live-wire observations as the authority for actual backend behavior. Turn the findings into a concrete implementation recommendation for Phoenix, with reproducible fixtures and measurements rather than inference from public platform-API documentation.

## Source Baseline

Pin and record the exact upstream `openai/codex` commit examined. Trace the full request and response path for GPT-5.6 models, including:

- model metadata for Sol, Terra, and Luna;
- `ResponsesApiRequest` and WebSocket request serialization;
- ChatGPT authentication and endpoint selection;
- Responses Lite headers and request shaping;
- stable thread-derived `prompt_cache_key` generation and overrides;
- HTTP versus WebSocket behavior and fallback;
- normal turns, retries, continuation, and remote/manual/automatic compaction;
- usage-event parsing and token aggregation;
- tests and snapshots that assert prefix identity or cache-key continuity.

Do not treat sample documentation embedded in the upstream repository as proof of runtime client behavior. Distinguish code that is executed, tests that pin it, documentation, and backend behavior observed on the wire.

## Investigation Plan

### 1. Reconstruct upstream runtime behavior

Produce a request-lifecycle map from user turn to serialized ChatGPT-backend request. Identify every field or header that can affect cache routing or prefix identity, including instructions, tools, reasoning controls, service tier, client metadata, item IDs, Responses Lite headers, and transport-specific fields.

Determine exactly how codex-rs keeps prefixes stable across:

- successive tool loops;
- per-turn configuration or permission changes;
- retries and HTTP fallback after WebSocket failure;
- conversation compaction;
- model-family transitions;
- review/guardian sessions with cache-key overrides.

### 2. Capture controlled live-wire evidence

Using a test account/environment authorized for Codex authentication, capture sanitized request and response shapes for GPT-5.6 Sol/Terra/Luna without logging bearer tokens, cookies, private prompt content, or account identifiers. Prefer an instrumented local build or existing safe debug hooks over a generic TLS interception proxy.

Run controlled paired requests with long, deterministic prefixes (at least the documented cache eligibility threshold) and one variable suffix. Record:

- requested model and server-reported model;
- transport and Responses Lite state;
- stable cache key presence;
- usage `input_tokens`, `cached_tokens`, and any `cache_write_tokens`;
- latency to first token and total latency;
- whether retries/fallback preserve the same cache cohort;
- any backend validation errors.

### 3. Probe cache-field support safely

In an isolated harness—not a production Phoenix conversation—test the ChatGPT Codex endpoint with the smallest controlled matrix needed to determine support for:

- baseline stable `prompt_cache_key` only;
- `prompt_cache_options` in implicit mode with `ttl: "30m"`;
- valid explicit breakpoints on supported message content;
- explicit mode without breakpoints;
- invalid placement on `function_call_output` as a negative control;
- HTTP and WebSocket request forms where both are available.

Classify each field as accepted-and-effective, accepted-but-unobservable, ignored/stripped, or rejected. An HTTP success alone is not evidence of caching effectiveness; require usage or repeat-request behavior consistent with a cache hit.

### 4. Compare Phoenix with upstream

For equivalent deterministic conversations, compare Phoenix Codex-auth requests against upstream codex-rs semantically and byte-wise for the cacheable prefix. Explain each divergence and whether it is required, neutral, or cache-damaging.

Pay particular attention to:

- system/instructions placement;
- tool ordering and schema serialization stability;
- text versus structured message content;
- tool-call and tool-result ordering;
- stable `prompt_cache_key` across stale-result clearing sweeps;
- whether Phoenix's request-only compaction causes one rewarm followed by stable hits;
- fields that change every turn before the intended cache boundary.

### 5. Measure optimization candidates

Measure at least these candidate strategies over repeated runs:

1. Upstream parity: stable cache key and automatic/implicit backend behavior only.
2. Phoenix stable-key behavior with stale tool-result clearing disabled and enabled.
3. Explicit cache options/breakpoints, only if the Codex backend demonstrates support.
4. Any prefix-stability fixes discovered during semantic/byte comparison.

Use raw per-run observations. Separate cold writes, warm reads, post-tool-loop requests, and post-compaction rewarming. Report cache-read ratio, cache-write tokens if available, uncached input tokens, first-token latency, and estimated cost/usage impact. Do not average away cold/warm transitions.

## Safety and Correctness Constraints

- Never commit credentials, cookies, authorization headers, account IDs, raw private prompts, or unsanitized wire captures.
- Do not infer ChatGPT-backend support from the public platform API contract.
- Do not infer effectiveness merely because a field is accepted.
- Preserve the direct-platform behavior delivered by task 54009 while investigating Codex.
- Keep provider/backend capability distinctions structural. If Codex supports a different subset, model that subset explicitly rather than reusing a broad boolean.
- A capability gap must be logged or rejected, never silently dropped.
- Token accounting must preserve Phoenix's non-double-counting context-window invariant.
- If upstream changes during the investigation, retain the pinned baseline and separately document the newer delta.

## Deliverables

- A pinned upstream source audit with stable symbol references and a request-lifecycle diagram.
- Sanitized, reproducible live-wire fixtures or a harness covering the support matrix.
- Raw cold/warm/tool-loop/post-compaction measurements for Codex-auth GPT-5.6 models available to the test account.
- A semantic and cache-prefix comparison between upstream codex-rs and Phoenix.
- A decision table for each cache feature and backend/transport combination.
- A recommended Phoenix implementation plan, including whether to retain, narrow, or remove the Codex gate.
- Follow-up implementation task(s) for confirmed optimizations; do not fold speculative production changes into the investigation.
- Spec corrections for any existing statement contradicted by observed behavior.

## Acceptance Criteria

- [ ] The exact upstream codex-rs revision and relevant runtime symbols are recorded.
- [ ] The ChatGPT/Codex request path is traced through model metadata, request construction, authentication routing, Responses Lite, HTTP/WebSocket transport, compaction, and usage parsing.
- [ ] Stable-key behavior is verified across normal turns, tool loops, retries/fallback, and compaction.
- [ ] A sanitized live probe determines whether cache options and explicit breakpoints are rejected, ignored, or effective on the Codex backend.
- [ ] Cache-read and cache-write usage fields returned by the backend are documented for each tested GPT-5.6 model and transport.
- [ ] Phoenix and upstream request prefixes are compared with every material divergence classified.
- [ ] Optimization candidates are measured with raw cold/warm and post-compaction samples.
- [ ] The recommendation states whether the Codex gate should remain, change, or be removed and cites direct evidence.
- [ ] Any implementation work is captured in scoped follow-up tasks with tests and spec obligations.
- [ ] No sensitive authentication or prompt data is present in committed artifacts.
- [ ] `./dev.py tasks validate` passes.
