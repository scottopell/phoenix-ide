# Specify the serial tool-dispatch lifecycle

Correct Bedrock's active-call/remaining-queue model and specify the existing runtime-owned preparation, permission denial, task launch, cancellation, and completion-acceptance boundary in Allium. Reuse the originating-request/schema, checked-call permission, tool-round identity, and sub-agent authorities without implementing new behavior.

Acceptance: explicit queue advancement and current-tool identity; a bounded dispatcher lifecycle with generation/state/identity guards and sender-loss settlement; current side-effect ordering and intercepted-tool scope remain accurate; no runtime/schema/wire/scheduling changes; applicable spec checks pass and an independent adversarial review challenges self-consistency and code agreement.

## Source/spec trace evidence

- Two-call round: `handle_core_llm_response` selects the first call and stores only the tail; `handle_core_tool_complete` advances active/remaining once, then atomically checkpoints the final result before the next model request. Bedrock now states those queue updates and matching-result guards explicitly.
- Originating policy unavailable: `dispatch_tool_execution` returns a paired `EUNAVAIL` event before `begin_dispatch`; failed request-policy or legacy-scope reads propagate an executor error without tool launch.
- Gate denial then next call: ordinary preparation has broadcast the start timestamp, opened the generation, and assembled context/metrics before `DenyGate::check`; denial forwards a completed error without a tool task. Its accepted completion advances Bedrock normally, and the next ordinary call opens a new generation.
- Launch and late revocation: the checked payload transfers into the task, while `execute_tool_to_outcome` rechecks current callable definitions and exact originating schema before `ToolExecutor::execute`; the live MCP boundary consumes that same schema-bound checked call.
- Cancel plus real completion: `AbortTool` signals the token without generation invalidation. An already-produced real completion may still reach `CancellingTool`, which checkpoints synthetic cancellation results for current and queued calls. Leaving cancellation releases the retained handle without cancelling or invalidating it again.
- Forced backstop and late reused ID: `handle_cancelling_tool_timeout` invalidates generation and aborts the retained task before its direct synthetic `ToolAborted`; the receiver's eventual sender-loss outcome remains tagged with the old generation and cannot settle a later call even if its ID is reused.
- Sender loss and wrong identity: `forward_tool_outcome` provides Failed for the expected identity, or normalizes a foreign outcome through `for_invocation`, before generation-tagged forwarding. Generation and reducer state/identity gates remain separate.
- Fatal closed fence: runtime admission closure suppresses semantic work even for already-queued current-generation completions; in-memory teardown signals cancellation, invalidates generation and aborts. No persistence receipt or database cleanup is fabricated.
- Bash wait fallback: task-only abort invalidates the original receiver before a direct Failed outcome for the expected wait identity; it does not cancel the represented process or reset the absolute persistence cutoff.

## Validation checkpoint

- Baseline and edited Bedrock/permissions/LLM/subagent Allium checks report zero error diagnostics; existing warnings remain unchanged. The new dispatcher file has zero warnings when checked with its imports.
- Default check plan selects only `allium`, `spec-anchors`, `spec-shape`, and `task` for this spec/task-only change. Runtime/UI suites are not selected or claimed rerun.
- Independent adversarial review is pending on the immutable implementation commit.

## Review corrections

- The initial isolated model reviews found disconnected event aliases and incomplete new-field exit obligations. The dispatcher now consumes Bedrock's actual start/cancel events and emits disjoint Bedrock completion/abort events. Wait/cancellation deadline retirement is acknowledged before synthetic settlement and any next dispatch; the new active/cancelling identity fields clear on generic restart exits.
- A subsequent model review found the cancellation-retirement acknowledgement depended on an ordinary invocation even for an intercepted selected call. Retirement now operates on the generic runtime slot, invalidates its generation and aborts any retained handle even when no ordinary invocation exists. Matching ordinary projections retire if present. The acknowledgement never waits for task cooperation.
- Manual composition traces cover wait A generation 1 → retirement generation 2 → one Failed settlement → queued B generation 3; an old generation-1 receiver is stale. A selected intercepted spawn → user cancellation → deadline has the same unconditional slot-retirement acknowledgement, then exactly one cancellation checkpoint, without inventing an ordinary tool task.
- The retirement handshake initially exposed two local unreachable-trigger findings for the cross-file acknowledgements. Bedrock now declares the real owned-executor acknowledgement surface with exact-conversation/request/call obligations; it adds no fake emitters or independent requests. The 41 pre-existing Bedrock findings were independently reproduced on the clean base and remain the accepted analysis baseline.
- The first default local check passed all five executed steps (task, Allium, spec shape, managed-Python dev.py tests, spec anchors), with nine source-gated steps skipped. Its static spec scan preceded later review corrections; current direct Allium checks cover those corrections, and a fresh default check will qualify the final semantic snapshot.

## Out-of-scope observation

Permissions Allium still includes the normative denial-count/escalation target while its executive marks that behavior unimplemented. The new reusable checked-call contract deliberately excludes that separate requirement; no escalation work is commissioned here.
