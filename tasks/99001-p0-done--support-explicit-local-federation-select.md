# Support explicit instance:local across the federated tool surface

Owner: Codex in /Users/scottopell/.codex-workspaces/worktrees/ios-0a633c26-2012-439a-98ea-acae8d9f135f/phoenix-ide. User approved the proposal and commissioned implementation, adversarial review, publication and a review-clean PR. Do not hand off this fix to the blocked Global Coordinator.

## Observed journey and production evidence

On 2026-11-02 Global transcript cc641b4c-880b-40d5-91da-d8d493888143 repeatedly invoked send_conversation_message with instance:"local". Production 0.13.1 at 5301a90ae4ccbe6634765bc820de9c478e0729af returned "peer connection not found" seven times between 22:01:03 and 22:03:12 UTC. The user's instruction at sequence 3217 explicitly requests supporting this trained model behavior across federated tools. Sequences 3233/3234, 3242/3243, 3251/3252 and 3260/3261 show the attempted implementation handoff itself failing. Nothing was delivered by those calls. Read-only inspection at 22:03:14 state timestamp shows Global idle; no production messages, restarts, DB edits, or deployments were performed by this investigation.

## Verified failure model and interaction map

Tool JSON -> optional String instance -> each handler treats every Some as remote -> api::federation::resolve_saved_peer_id -> UUID/name lookup -> PeerNotFound. The remote selector resolver trims its input; no saved peer named local exists on this host. This is a routing-contract gap, not evidence of transport failure. Federation requirements REQ-FED-001 and Allium selection rules also define every present selector as remote and must change with the implementation.

The complete deployed and current-main optional-instance inventory is send_conversation_message, FederatedReadConversation/read_conversation, ResolveReference/resolve_reference, and coordinator_tools::goals::GoalTool/read_goal. remote_query_database uses a required typed instance_id UUID and is a distinct remote-only contract. LocalReadConversation for ordinary conversations has no optional instance capability; do not broaden authority incidentally.

Current worktree starts clean at detached 27c514034be4cb68772880f95e2fddd716911a69, predating federation. origin/main fetched at 9441cd2824324e53697df88f4e9db07f6de3980e still contains the bug; owning files have no diff against deployed 5301a90. Begin implementation on an owned branch from refreshed main in this same worktree, preserving this task. Do not reset or move another checkout's local main.

## Proposed scope

Introduce one shared typed local/remote selector at the optional-instance tool boundary. Omission and the exact case-sensitive string "local" produce the same Local variant and dispatch through the identical existing local services. Other strings remain remote peer selectors with existing validation and error behavior. Preserve existing per-tool null/type-validation behavior unless an explicit contract change is warranted; do not introduce fuzzy local aliases or turn invalid values into local execution. Keep remote-only peer resolution remote-only.

Reserve exact local as a tool selector. User refinement: disallow local as a peer name because it is confusing. Enrollment and saved-connection creation/replacement require a shared validated PeerDisplayName that trims whitespace and rejects blank names and ASCII case variants of local before any mutation. No live peer rename, enrollment, credential, trust or persisted-label rewrite. Document the policy in federation requirements, Allium, the shared ADR chain, executive evidence and all four tool descriptions/property descriptions. Keep this fix separate from durable-watch PR1015 and unrelated federation lifecycle work.

Starting symbols: coordinator_tools::{FederatedReadConversationInput,ResolveReferenceInput,SendConversationMessageInput,present_instance_selector}; coordinator_tools::goals::{ReadInput,GoalTool::read}; api::federation::resolve_saved_peer_id. Normalize before remote-only validation/dispatch so explicit-local reads keep local paging/message-anchor behavior and sends retain local provenance/admission/retry semantics. Goal authorization must remain before dispatch, including version waits.

## Acceptance and validation

- Reproduce explicit-local failure in a focused tool test before implementation.
- All four optional-instance tools have omitted-versus-explicit-local parity tests through their actual handlers, preserving local output shapes and permissions.
- Sends cover stable and exact targets, delivery/steering, trusted source-call provenance, payload-bound message identity retry, self/Coordinator/sub-agent restrictions, and no duplicate mutation.
- Read/resolve tests cover local-only capabilities, exact-message references and local pagination; do not impose remote-only restrictions on explicit local.
- Goal reads cover ordinary and wait paths, history cursor, authoritative snapshots and Coordinator authorization.
- Unknown, ambiguous and unavailable non-local peers remain errors with no local fallback; self instance UUID remains governed by the existing explicit-peer contract.
- Enrollment and peer import reject local and case/whitespace variants with a clear input error, without revoking an existing credential or replacing a saved connection. Persistence write methods require the validated name type.
- Test exact/case/whitespace/empty/null/malformed selectors without accidentally expanding accepted local spellings or breaking existing input validation.
- Run focused tests, owning crate checks, spec authoring preflight/Allium validation, then appropriate ./dev.py check lanes. Inspect tool publication/reconstruction to ensure the deployed handler applies to the existing Coordinator, not only newly created conversations.
- Commit and publish a reviewable fix on the owned branch after implementation. Production deployment and live behavioral acceptance are separate steps, not claimed by source qualification.

## Limits and unknowns

No universal tool retry redesign, historical message rewriting, manual DB repair, remote SQL alias, new remote mutations, new worker/executor, durable-watch implementation, or production change is part of this proposal. The model's continued generation of local is observed; training is the user's explanation rather than a claim we can independently verify. Production acceptance remains outstanding until a qualified build is installed and a real explicit-local action succeeds.

## Implementation and verification

Implementation is based on main 9441cd2824324e53697df88f4e9db07f6de3980e on branch fix/explicit-local-instance in the original worktree. All four handlers deserialize a shared InstanceSelector; its remote payload has no public string constructor. The local variant reuses the existing local services. All four tools share selector schema guidance. Requirements, Allium and ADR-216 describe the reserved literal, peer-name admission and preserved remote contracts. PeerDisplayName is shared by HTTP enrollment/import inputs and the database write services; historical stored display labels remain read data.

Before the fix, the real send, read/resolve and Goal regression tests failed on remote routing; the input-validation regression passed. At the initial selector checkpoint all five explicit_local tests passed, including delivery and steering, retry/payload conflict, provenance, target restrictions, local paging/message anchors, Goal history/waits and authorization, reserved-name precedence and UUID remote routing. The subsequent user-requested name ban replaces the confusing-peer fixture with ordinary peer selection and adds constructor/JSON and HTTP rejection-without-replacement coverage.

Validation commands:

- cargo test -p phoenix_ide --lib explicit_local -- --nocapture: 5 passed.
- ./dev.py check --lanes cargo-fmt,allium,ast-grep,spec-anchors,task: all 7 checks passed.
- ./dev.py check --lanes spec-shape: shape validation and the Python integration suite both passed (the suite also builds the app and exercises seed fixtures).
- The initial broader Coordinator run passed 44 tests with one ignored private benchmark and one macOS Bash output failure. The isolated failure passed with PHOENIX_BASH_LAUNCH_TEST_EXE set to the native app binary, as in CI; the configured full suite then passed 45 tests with one ignored benchmark. This is a local invocation prerequisite, not a product-code repair.
- After the name ban: both core name-validation/JSON tests and the real HTTP rejection-without-replacement regression pass; 12 federation persistence tests pass; the configured Coordinator suite again passes 45 tests with one ignored benchmark.
- Isolated Phoenix adversarial review of immutable base 9441cd2824324e53697df88f4e9db07f6de3980e to source head 2002aae8c43b1d6588fdc91ff4a466a0e1bc6053 reports no actionable findings. It covers all changed routing, naming, authority and persistence boundaries plus runtime tool reconstruction. Reviewer started no native builds and sealed findings before external PR feedback.
- Source implementation is complete. Full federation API tests, Clippy and the remaining check lanes are running; hosted CI and PR review are the remaining publication qualification. Their state belongs to the PR rather than being inferred from this task filename. No production deployment or live acceptance is claimed.
