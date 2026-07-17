# Make the Global Coordinator an Actionable Conversation Console

## Why we are building this

The global Coordinator should let a user survey, unstick, and nudge existing Phoenix conversations through natural language. Today it can inspect global work but cannot communicate with the conversations it analyzes, so the user still has to open each conversation, read it, decide what to say, and send messages manually. It also makes the model call `list_open_work` to regain basic orientation even though Phoenix already owns a deterministic current-work projection.

Build a chat-first global surface where the Coordinator receives compact current work context automatically, selectively reads relevant conversations, and has exactly one write capability: send a text message to an existing conversation. The existing chat acceptance path determines whether that message starts immediately or enters the steering queue.

This is an open-ended cross-conversation console, not a manager for one global objective. Users may survey unrelated projects and conversations in the same Coordinator chat.

## User experience

The main `/global` surface is the normal durable Coordinator transcript and composer. A smaller utility pane shows conversations whose **current state** needs attention and provides a deterministic Find Work control. It replaces the existing metadata-heavy Fleet presentation.

Representative requests:

- “Survey blocked work and tell me what actually needs my input.”
- “Check the auth and UI conversations; nudge anything that can proceed.”
- “Tell every active conversation touching the API to use the decision from the auth discussion.”
- “Find conversations that look stalled, inspect the relevant recent context, and ask for an update where useful.”

A Coordinator response reports evidence and committed actions per target:

```text
I inspected four relevant work items.

✓ Sent an update to @work/auth-api
✓ Queued a steering message for @work/oauth-ui; it is currently busy
✗ Did not message @work/old-ui; its current conversation is terminal

@work/release still needs your direct input, so I left it unchanged.
```

The Coordinator may issue several independent message calls from one natural-language request. There is no batch-action transaction: every target has its own result, and partial success must be reported accurately.

## Product requirements and specifications

Update `specs/global-recall/requirements.md` and `specs/global-recall/executive.md` before changing implementation. Leave the timeless requirements free of task/PR references and run the pre-flight checklist in `specs/AUTHORING.md` before pushing spec changes.

Revise the global Coordinator contract to require:

1. The Coordinator remains the single durable Phoenix-wide conversation and uses the normal transcript, composer, streaming, continuation, and persistence runtime.
2. Phoenix supplies a compact deterministic projection of current work to each Coordinator LLM turn without requiring a `list_open_work` tool call.
3. The projection describes current state only; it does not claim to be an exact event history or an exact delta since a prior turn.
4. The Coordinator may search global history, read bounded conversation content, resolve stable references, and query the complete deterministic open-work projection when the compact context is insufficient.
5. The Coordinator may send a text message to an existing non-Coordinator conversation. The existing user-message acceptance rules decide whether the message runs immediately, is queued as steering, or is rejected.
6. The action returns the resolved durable target and the committed per-target outcome. It must not claim that the receiving agent understood, acknowledged, or completed the instruction.
7. The Coordinator has no other mutation capability in this phase.
8. The global UI is chat-first and presents a small current-attention utility and deterministic Find Work control instead of the existing Fleet inventory.
9. Current-attention items navigate to their owning conversation, where the user handles questions, approvals, and errors using the existing native controls.
10. Ordinary conversations do not receive Phoenix-wide read tools or the cross-conversation message tool.

REQ-GR-007 currently defines the Coordinator as read-only. Replace that rule with the narrow text-message authority above while retaining its prohibitions against filesystem, repository, task, workspace, creation, approval, and other lifecycle mutation tools. Update the user story, rationale, and executive current-reality text so they describe an actionable conversation console rather than a read-only analyst.

## Architecture

### 1. One narrow cross-conversation action

Add one Coordinator-only tool with a name such as `send_conversation_message`. Its input is:

```text
target: @work, @conv, app-local conversation link, or conversation id
message: non-empty text
message_id: client/tool-generated stable UUID for idempotent retry
```

Its output is a typed semantic result rendered clearly for the model:

```text
Delivered { target, conversation_id, message_id }
QueuedAsSteering { target, conversation_id, message_id }
Rejected { target?, conversation_id?, reason_code, message }
```

Use an enum or equivalent typed result internally so delivered, steering, and rejection cannot be confused. Do not return `queued: true` as the sole semantic distinction: the existing HTTP response uses that field for both accepted paths and requires `steering` to distinguish them.

The tool accepts text only. It does not accept images, files, skills, filesystem references, or user-agent metadata.

Register it only in the Coordinator registry assembled in `runtime.rs` / `coordinator_tools.rs`. Keep that registry builtin-only and continue excluding MCP, bash, filesystem, browser, task, project, creation, and ordinary lifecycle tools. Update the `ToolRegistry::coordinator` documentation in `phoenix-tools` so it describes this bounded global action rather than a read-only registry.

### 2. Reuse the authoritative chat/steering path

Do not make the tool call an in-process HTTP handler and do not duplicate `send_chat` state routing in the tool.

Extract the text-message acceptance and dispatch behavior currently embedded in `api::handlers::send_chat` into a shared application service with typed input and result. Both the HTTP handler and Coordinator tool call that service. Keep the existing authorities and behavior in one path:

- message-id idempotency against persisted messages;
- steering-queue idempotency;
- live runtime state as authority when a handle exists;
- stable DB rejection states when no runtime exists;
- runtime materialization where the existing path requires it;
- `check_user_message_acceptable` semantics;
- queue busy/cancelling targets as `Event::SteerMessage`;
- enforce the existing steering depth limit;
- dispatch acceptable targets as `Event::UserMessage`;
- preserve existing persistence, broadcast, and runtime behavior;
- preserve existing PR auto-fix baseline recording behavior where applicable.

Keep attachment validation/upload and browser-specific request decoding in the HTTP adapter. The shared service should accept a prepared text-only message for the Coordinator path and the prepared expanded message/attachments needed by the existing HTTP path without introducing two copies of acceptance logic.

The action is successful only when the existing path has accepted the event or steering entry. Report `Delivered` for normal `UserMessage` acceptance and `QueuedAsSteering` only when the steering queue accepted it. Return stable rejection codes for not found, terminal/context-exhausted/approval/question states, full steering queue, and other existing conflicts.

The Coordinator does not gain a second way to answer `AwaitingUserResponse`; the ordinary message path currently rejects that state. It should report the rejection and direct the user to open the owning conversation.

### 3. Target resolution and authorization

Resolve `@work` references to their current/latest conversation, preserving continuation-chain identity. Reuse the typed internals behind global reference resolution rather than parsing the human-formatted output of `resolve_reference`.

Before dispatch:

- require that the caller is the Coordinator runtime by construction through the dedicated registry/service capability;
- reject unresolved or ambiguous syntax rather than guessing;
- reject archived, deleted, or otherwise unavailable targets through the authoritative conversation state/lifecycle checks;
- reject the Coordinator conversation and every member of its continuation chain as targets;
- do not silently retarget a terminal current conversation to a different historical member;
- log origin Coordinator id, resolved target id, message id, and delivered/steering/rejection outcome without logging message text at elevated levels.

Do not expose a new public “send as Coordinator” HTTP endpoint. The model tool is the only new action surface; normal clients continue using the existing conversation chat endpoint.

### 4. Automatic current-work context

At each Coordinator LLM dispatch, build a bounded current-work capsule from the same deterministic projection used by `GlobalReadService::open_work`. Attach it as non-cached, turn-current system context after the stable cached Coordinator system prompt; do not interpolate changing fleet data into the cached prompt prefix.

The capsule should prioritize, in order:

1. work currently needing user attention or in an error/recovery state;
2. active work;
3. recently idle open work;
4. aggregate counts and an explicit truncation notice when the bounded capsule omits items.

For each included item provide only the fields useful for selection and orientation: stable `@work` reference, project, concise title, current state, mode, recency, and strongest inclusion/attention signals. Avoid worktree paths, branch metadata, member counts, and other audit detail unless needed to disambiguate. Keep `list_open_work` available for a complete or paginated inventory.

Give the capsule an explicit heading and instruction that it is current deterministic state, not transcript evidence and not a complete history. The Coordinator prompt should instruct the model to:

- use the capsule for initial orientation instead of reflexively calling `list_open_work`;
- inspect only conversations relevant to the user’s request;
- use global search when locating a decision or topic across history;
- cite source-specific historical claims with existing stable references;
- message only when intervention is useful;
- report each target’s committed result without implying semantic acknowledgement;
- never imply background monitoring, because it runs only on user turns.

Keep capsule construction pure/bounded enough for focused tests, including deterministic ordering and truncation.

### 5. Chat-first global UI

Refactor `CoordinatorPage` and its colocated CSS so the Coordinator conversation is visually dominant on desktop. Replace the existing Fleet pane, expandable audit rows, copy-reference-first actions, and project-grouped inventory with a narrow utility pane containing:

- **Needs attention:** a current-state projection of open work whose existing state/signals indicate a question, approval, error, recovery, or other current attention need;
- **Find work:** deterministic search/filter over current open work, returning compact rows with title, project, state, recency, and stable navigation to the owning/current conversation;
- an explicit refresh/freshness indicator.

This utility is a projection of current state and has no read/unread, seen, acknowledgement, resolution, event history, or outcome-retention model. An item disappears when the underlying current projection no longer qualifies. Selecting an item navigates to the owning conversation; do not reproduce question, approval, retry, cancel, or dismiss controls inside the utility pane.

For Find Work, filter before pagination so matching work is not missed merely because it falls beyond the first 100 unfiltered items. Extend the deterministic global open-work query/service with bounded query and filter parameters or provide a dedicated read endpoint over the same projection. Do not involve the LLM, semantic search, persistent saved views, or a cache.

On compact layouts preserve the existing ability to switch between conversation and utility views, but rename/reframe the utility as Attention/Find Work rather than Fleet. Refresh the utility on page entry, explicit refresh, window focus, and completion of a Coordinator turn using existing UI/runtime signals. Do not add a new timer poller or a new SSE protocol for this phase.

### 6. System prompt and language variants

Update both Coordinator prompt variants in `phoenix-core/src/llm_language.rs`. They must no longer say the Coordinator is read-only or cannot change other conversations. They must describe exactly the bounded permission: it may send text through the existing message/steering path to existing non-Coordinator conversations, and may not claim any other mutation or background activity.

Update prompt tests in `system_prompt.rs` and registry tests so every supported language receives the same capability boundary.

## Correctness invariants

- Only the Coordinator registry contains the cross-conversation message tool.
- The shared chat acceptance service is the sole authority for immediate versus steering versus rejection behavior.
- Retrying the same `message_id` does not create a second persisted message or steering entry.
- One tool invocation targets exactly one resolved current conversation.
- Several tool invocations in one Coordinator turn are independent; one failure does not alter or conceal another result.
- A tool result records acceptance, not recipient understanding or completion.
- No Coordinator-chain member can target itself or another Coordinator-chain member.
- Current-work context and the utility pane derive from the same open-work semantics; neither becomes a second persisted representation.
- Current attention is reconstructible from current state and stores no historical inbox state.
- Ordinary conversation tools and permissions are unchanged.

## Implementation sequence

1. Update the global-recall requirements and executive documentation, including the transparency contract and the bounded action authority.
2. Extract and test the shared text-message acceptance/dispatch service without changing existing HTTP chat behavior.
3. Add typed global target resolution suitable for both read references and message dispatch.
4. Add the Coordinator-only message tool, registry boundary, logs, prompt language, and tool tests.
5. Add the bounded current-work capsule to Coordinator LLM request assembly and test ordering, truncation, and prompt-cache placement.
6. Replace the Fleet UI with the chat-first current-attention and Find Work utility, including responsive behavior and API filtering before pagination.
7. Add integration tests covering immediate delivery, busy steering, idempotent retry, queue-full rejection, terminal/question/approval rejection, target continuation resolution, self-chain rejection, partial multi-target results, and absence of the tool from ordinary conversations.
8. Run code generation if any typed wire surface changes, then run focused Rust/UI tests and `./dev.py check`.
9. Exercise the complete user journey in the running app: ask the Coordinator to survey several work items, verify it begins from injected current context, selectively reads relevant conversations, sends one immediate message and one steering message, reports both accurately, and shows the updated current-state utility without manual Fleet polling.

## Verification scenarios

### Survey without redundant polling

Given several open work items, send “Survey blocked work and tell me what needs intervention.” Verify the first LLM request already contains the bounded current capsule and that the Coordinator does not need `list_open_work` merely to discover basic current state.

### Immediate message

Target an idle conversation. Verify exactly one user message is accepted through the normal runtime path, the target begins processing normally, and the tool reports `Delivered` with the resolved id/reference.

### Steering message

Target a busy conversation. Verify exactly one steering item is queued, the tool reports `QueuedAsSteering`, the target later consumes it through the existing queue behavior, and no immediate overlapping turn starts.

### Idempotent retry

Execute the same target/message/message-id twice for both persisted-message and queued-steering cases. Verify no duplicate is created and the retry returns the previously committed semantic outcome.

### Rejections

Verify stable, useful rejection results for unknown reference, Coordinator-chain target, archived/deleted target, terminal/context-exhausted target, awaiting-question/approval target, and full steering queue. No rejected case may be reported as sent or queued.

### Natural-language fan-out

Ask the Coordinator to notify several matching work items. Verify it resolves and invokes the singular tool once per target, reports delivered/steering/rejected separately, and does not imply atomic batch success.

### Permission isolation

Verify ordinary Direct, Explore, Work, Branch, and sub-agent conversations never receive this tool. Verify the Coordinator still does not receive filesystem, shell, MCP, task, creation, approval, or lifecycle mutation tools.

### Utility semantics

Verify the utility includes current attention states, removes an item after its underlying state no longer qualifies, finds matches beyond the first unfiltered page, navigates to the current owning conversation, and retains no unread/history state after reload.

## Deferred phases

After the Durable Workflows independent-consumer persistence boundary has landed and stabilized, a separate phase may add durable attention/outcome history, observation cursors, exact changes-since-last-turn context, and desktop notifications sourced from that durable attention model.

After the protocol-agnostic conversation-creation acceptance boundary has landed and stabilized with typed idempotent receipts and shared capability projection, a separate phase may allow the Coordinator to create Explore work. These dependencies are not part of this build.
