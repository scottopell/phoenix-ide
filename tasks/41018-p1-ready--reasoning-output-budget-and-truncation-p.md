# Reasoning-heavy Chat Completions can exhaust output before any tool call

## Verified investigation
On deployed d69cde675, GLM5.3 coding smoke task failed three first-turn attempts with finish_reason=length mapped to InvalidResponse. Local prod llm_request_metrics records reasoning-only generation, zero visible-text events, durations 79810/105021/67204ms; failed child has no turn_usage rows. A successful later GLM turn recorded 15669 output tokens including15625 reasoning tokens; another14170 including11179. Counts do not prove failed token totals (usage unavailable).

Executor chooses desired16384 tokens unless effective effort is Xhigh/Max, then64000; min with configured output ceiling. Installed GLM config max_output_tokens131072 has no effort metadata => NativeUnknown, no reasoning_effort sent =>16384 budget. Gateway docs third-party OSS models page7052100418 lists high default on Baseten and max default on Databricks. Do not assert exact failed physical provider/default or captured raw request; evidence reconstructs request budget from unchanged code/config, not historical body.

## Failure model
Model capability ceiling is not desired request budget. External reasoning model unknown native default falls into small-budget policy. Even known high still16384. Length classified InvalidResponse retries unchanged prompt/budget up to3 total attempts; private reasoning discarded as intended. Streaming returns on length before later usage trailer; error type carries no usage even when usage already present. Missing usage is unknown cost, not zero.

## Proposed bounded work (policy decision needed)
- Resolve desired output budget separately from capability ceiling/effort vocabulary; use identical resolved budget for request and context reservation. Decide per-model configured budget vs code policy; do not advertise unsupported xhigh just to get headroom, do not silently reinterpret max_output_tokens globally.
- Give output exhaustion an explicit typed recovery policy rather than generic unchanged-budget retries (stop/actionable vs bounded escalation with cost/context constraints). Update normative ChatLengthFinishRejected rule before changing current specified InvalidResponse behavior.
- Preserve provider usage available on failed attempts, boundedly consuming usage trailers without executing partial tool calls or exposing private reasoning. Retain unknown usage honestly.
- Deterministic tests: budget/effort/ceiling table, external unknown effort serialization, SSE reasoning->length->usage/DONE (same and different buffers), usage missing, JSON length+usage, no partial tool execution, transient invalid response versus truncation retry behavior.

## Scope boundaries
No direct gateway field rename max_tokens->max_completion_tokens justified by evidence. No global effort assumption across logical provider routing. No user env or production changes during investigation. See runtime executor output budget, LlmRequest::reserved_output_tokens, registry external effort validation, ChatCompletionsAccumulator, retry policy.
