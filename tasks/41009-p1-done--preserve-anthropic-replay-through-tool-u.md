# Preserve active Anthropic replay through tool unavailability

## Observed journey and evidence
An assistant requested an MCP tool after provider tool search. The MCP client returned an authentication-related shutdown error. The next request refreshed the callable catalog, stripped the now-unavailable invocation from projected history, and failed the exact Anthropic replay-owner check before provider I/O.

Read-only SQL confirms messages.content exactly equals the corresponding active_provider_replay_state response-set public_content (five blocks). The owner was neither deleted nor rewritten in storage. The active replay row remains present. The original runtime retained both conflicting paths.

## Failure model and boundary
Request preparation refreshes callable tools, runs strip_unavailable_tool_blocks, then separately loads provider replay. That normalizer drops unavailable tool uses and paired results and filters tool-search references. apply_provider_replay compares the projected assistant content with the original public_content and rejects the rewritten owner before provider I/O. Thus transient catalog unavailability can freeze an otherwise valid active exchange and also suppress its error-result context. InvalidRequest is non-auto-retryable; Error retains replay. Exact underlying MCP authentication cause is outside this task.

## Scope and acceptance
Preserve the normative specs/llm/requirements.md contract for exact owner-bound private replay and validated ordinals. Make active exchange projection and tool declaration policy coherent when catalog availability changes; distinguish ability to replay historical declarations from ability to execute new calls. Do not weaken equality validation, fabricate replay snapshots, silently discard private blocks, or pin unrelated prompt configuration. Validate Anthropic tool-reference requirements before choosing the narrow implementation.

Add an integration regression starting with private blocks plus server tool-search blocks and an MCP tool-use, persist a failed tool result, remove the tool from the live catalog, and build the next Anthropic request. Assert owner/public content and private block ordinals stay intact, the error result reaches the model, and no tool is re-executed. Cover retry/restart materialization with retained replay, plus settled history preservation and unaffected available-tool paths. Reconcile executive verification coverage as appropriate.

Risks: retaining historical declarations must not authorize unavailable executions; provider tool references and replay positions must remain valid. Existing tests cover normalization and replay rejection separately but do not prove this combined journey. No Slack auth repair, production DB surgery, restart, or deployment is included.

## Implementation scope
Separate durable retained declarations from current callable names. Persist anchored Anthropic native changes and exact owner-bound Responses output envelopes. Render restrictions for the selected route only, enforce withdrawals at dispatch, and atomically retire continuation state on settled provider changes. Restore missing legacy schemas only from authentic catalogs, with chronology-aware native rebaselining. Preserve private continuation through the chain QA consumer as well as the conversation runtime.

## Completion and verification
Implemented durable tool policy, lossless historical projection, route-specific wire controls, dispatch-time EUNAVAIL, private Responses envelopes, atomic switch settlement, and continuation incarnation checks. Regression coverage includes the original failed-MCP/private-Anthropic-owner journey through retry and restart, native anchor legality after new user input, phase-bearing Responses rounds without reasoning, and fresh Codex WebSocket contexts after provider switching. All 53 Allium models validate. Full CI passed on the implementation revision; final envelope coverage is gated on the PR checks. Independent adversarial review findings were repaired and covered with regressions.
