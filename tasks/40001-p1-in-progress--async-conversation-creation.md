# Async conversation creation with instant shell

## Goal

Make conversation creation return immediately with a durable, navigable conversation shell, then complete validation/provisioning/message dispatch asynchronously. This targets the known UX hang in the `/new` page, especially the common `new-conversation-in-worktree` flow, and establishes the pattern for later async task/fork approval work.

## Product decisions already made

- **User-visible guarantee:** instant shell first.
  - The client generates a conversation GUID.
  - The server accepts cheaply, persists a shell, and returns immediately.
  - The UI navigates to `/c/<client-guid>` before worktree/title/runtime startup completes.
- **Failure model:** failed record.
  - If async provisioning fails, keep a durable failed conversation record.
  - Show the original prompt/creation intent read-only.
  - Offer actions such as Delete and Start over.
  - Start over can route to `/new` prefilled from the stored creation intent; do not build branch/cwd/reference repair UX inside the conversation page.
- **Synchronous validation:** minimal accept.
  - Only validate cheap structural facts before returning: GUID/request shape, non-empty allowed input shape, known model/mode enum.
  - Filesystem/git/branch/worktree/reference expansion/attachment-finalization/runtime-start failures happen asynchronously and can produce a failed record.
- **Scope:** all conversation creation paths.
  - JSON create, multipart attachment create, direct, managed, branch, auto, seeded/task-viewer flows should converge on the async contract.
- **Queued prompt representation:** creation intent record.
  - Do not create a normal message row until async validation and expansion succeeds.
  - Persist raw creation intent/provisioning input separately from `messages`.

## Implementation shape

Introduce a durable conversation creation job/intent model:

```text
Conversation
  id = client-supplied GUID
  slug/title may be null or temporary initially
  state/presentation indicates provisioning, ready, or creation failed

ConversationCreationJob / intent
  conversation_id
  client_message_id
  request/intent fields: cwd, text, model, mode, base_branch, images, seed_parent_id, seed_label, attachment refs
  phase
  error
  timestamps
```

The async worker should:

1. Load pending creation jobs durably from DB.
2. Run the existing create-time work in the background:
   - cwd validation;
   - project/git detection;
   - branch/default-branch resolution;
   - managed/branch worktree creation;
   - taskmd hint snapshot;
   - attachment finalization;
   - `@file` and `/skill` expansion;
   - conversation metadata/title/slug update;
   - normal `UserMessage` creation/dispatch via runtime.
3. Mark the conversation ready/running on success.
4. Mark the conversation creation failed on error, preserving the intent and error.
5. Be restart-safe: accepted jobs must resume after server restart; do not rely only on an in-memory spawned task.

## Serial sub-agent execution model

The parent coordinator should own decomposition, review each phase, run QA, and land one PR. Implementation should be delegated serially to preserve context.

### Phase 1 — Backend schema and types

**Read first:**

- `crates/phoenix-ide/src/db.rs` and DB migration layout
- `crates/phoenix-ide/src/state_machine/state.rs`
- `crates/phoenix-ide/src/api/types.rs`
- existing conversation/message persistence helpers

**Modify:**

- DB migrations/schema code
- Rust DB/domain types for creation jobs/intents/phases/errors
- generated/API-facing types as needed

**Do not change:** frontend behavior or HTTP handlers beyond what is needed to compile type additions.

**Done when:** there is a durable, restart-readable representation of accepted async conversation creation jobs and provisioning/failed conversation presentation state.

### Phase 2 — Backend async create endpoint and job enqueue

**Read first:**

- `crates/phoenix-ide/src/api/handlers.rs` conversation creation section
- `crates/phoenix-ide/src/api/types.rs`
- `crates/phoenix-ide/src/conversation_cwd.rs`
- attachment create helpers in `handlers.rs`

**Modify:**

- create endpoint handlers/routes
- request/response types
- minimal synchronous validation
- multipart staging/enqueue path

**Do not change:** runtime provisioning internals yet except to keep the project compiling.

**Done when:** all create paths can accept a client-supplied conversation id, persist a shell + creation intent/job, and return quickly without awaiting git/worktree/title/expansion/runtime dispatch.

### Phase 3 — Backend creation worker/provisioning engine

**Read first:**

- current `create_conversation_with_id` implementation in `handlers.rs`
- runtime send-event path
- worktree helpers in `handlers.rs` / `git_ops`
- startup/runtime manager initialization code for background workers

**Modify:**

- new or existing backend worker module
- extraction of current synchronous create internals into reusable provisioning code
- job claiming/resume/error handling
- SSE or conversation update emissions as appropriate

**Do not change:** task approval/fork approval semantics in this phase.

**Done when:** pending creation jobs are processed asynchronously, survive restart, create normal user messages only after expansion succeeds, and mark failed records on error.

### Phase 4 — Frontend create flow and conversation page states

**Read first:**

- `ui/src/api.ts`
- `/new` page/components and create conversation hooks/store
- `ui/src/components/TaskViewer.tsx`
- conversation route/loading components
- conversation list/sidebar store code

**Modify:**

- client GUID generation for conversations
- async create API client
- navigation to `/c/<id>` before slug exists
- provisioning and failed-record UI states
- Start over action that pre-fills `/new` from stored creation intent
- task-viewer seeded create flow to use the same async contract

**Do not change:** unrelated conversation rendering or task approval UI except where needed for create flow consistency.

**Done when:** all creation entry points navigate immediately to the shell, show queued creation intent/progress, transition to normal transcript on success, and show failed record with Start over/Delete on failure.

### Phase 5 — Compatibility cleanup and generated types

**Read first:**

- legacy `/api/conversations/new` callers
- generated TS type workflow
- tests around conversation creation and SSE schemas

**Modify:**

- compatibility wrappers or call sites so old and new create paths do not diverge
- generated TS files via `./dev.py codegen` if wire types changed
- stale comments/spec executive notes if needed

**Done when:** there is one authoritative async creation path, with legacy endpoints either wrapping it or clearly migrated.

## Automatic QA phases

### QA-1 — Backend tests

Run focused Rust tests for conversation creation, DB job persistence, and runtime dispatch. Add tests for:

- duplicate client conversation id retry is idempotent;
- duplicate initial message id is idempotent or rejected deterministically;
- minimal accept returns even when cwd/branch is invalid, then worker marks failed;
- successful managed create eventually creates the normal initial user message;
- restart/resume processes accepted jobs.

### QA-2 — Frontend tests

Run relevant UI tests and add/update tests for:

- create flow generates conversation id and navigates immediately;
- provisioning shell renders queued intent;
- failed record renders read-only prompt and Start over/Delete affordances;
- seeded task-viewer flow uses async create semantics.

### QA-3 — Full project verification

Run:

```bash
./dev.py codegen
./dev.py check
```

Address generated-type drift and lint/test failures.

### QA-4 — Manual/dev verification

With `./dev.py up`, verify:

1. `/new` managed worktree create navigates immediately.
2. Successful create transitions from provisioning to normal conversation and starts the first message.
3. Intentionally invalid branch/cwd produces a durable failed conversation record.
4. Start over pre-fills `/new` from the stored intent.
5. Multipart attachment create uses the same shell/provisioning behavior.

## Out of scope for this task

- Making task approval async, except where create-flow infrastructure must be reusable.
- Making fork proposal approval async.
- Building in-conversation repair UX for failed provisioning.
- Performance tuning beyond removing create-time synchronous blocking from the user-facing request.

## Follow-up work expected

After this lands and tracing confirms behavior, reuse the same durable accept/provision/fail pattern for:

- `approve-task` with `start_fresh_work_conversation`;
- `approve-task` with `continue_here` if runtime/event dispatch is blocking;
- fork proposal approve/request-changes, where child ids are already deterministic from proposal ids.
