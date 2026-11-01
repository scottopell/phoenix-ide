# Reasoning-heavy Chat Completions can exhaust output before any tool call

## Verified investigation
On deployed d69cde675, GLM5.3 coding smoke task failed three first-turn attempts with finish_reason=length mapped to InvalidResponse. Local prod llm_request_metrics records reasoning-only generation, zero visible-text events, durations 79810/105021/67204ms; failed child has no turn_usage rows. A successful later GLM turn recorded 15669 output tokens including15625 reasoning tokens; another14170 including11179. Counts do not prove failed token totals (usage unavailable).

Executor chooses desired16384 tokens unless effective effort is Xhigh/Max, then64000; min with configured output ceiling. Installed GLM config max_output_tokens131072 has no effort metadata => NativeUnknown, no reasoning_effort sent =>16384 budget. Gateway docs third-party OSS models page7052100418 lists high default on Baseten and max default on Databricks. Do not assert exact failed physical provider/default or captured raw request; evidence reconstructs request budget from unchanged code/config, not historical body.

## Failure model
Model capability ceiling is not desired request budget. External reasoning model unknown native default falls into small-budget policy. Even known high still16384. Length classified InvalidResponse retries unchanged prompt/budget up to3 total attempts; private reasoning discarded as intended. Streaming returns on length before later usage trailer; error type carries no usage even when usage already present. Missing usage is unknown cost, not zero.

## Approved implementation scope
- Default ordinary and continuation requests to the declared model maximum; remove effort-keyed and 4k allowances. One concrete value drives requests and context reservation. No new budget override.
- Require explicit external maxima; fill verified built-in model limits. Preserve native Codex wire omission.
- Classify output exhaustion separately from context and malformed responses, with no automatic retry and user resumability. Never execute truncated tool calls.
- Specs precede code (ADR-144, REQ-LLM-004f/006c, provider Allium). Deterministic budget, state-machine, JSON/SSE and partial-tool tests.

## Deferred
Failed-attempt usage retention and bounded usage-trailer capture remain separate follow-up work. Existing absence of usage does not imply zero cost. No global wire-field rename or unverified effort default.

## Qualification
Core/LLM/state-machine suites and focused runtime suites; whole-repo check/CI and exact-head review must pass before merge. Production configuration/deployment is separate and is not changed by this task.
