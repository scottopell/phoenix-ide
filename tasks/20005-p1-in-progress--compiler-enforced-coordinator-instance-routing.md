# Compiler-enforced Coordinator instance routing

## Baseline and defect
At 9c5d471d, watch/unwatch/list-watched-conversations parse instance as Option<String>; exact "local" incorrectly selects the remote branch. The other five optional-instance tools use a shared Local/Remote parser but pass raw selectors or IDs into remote APIs, and registration cannot enforce their common contract. Recheck origin/main and the Federation owner's changes before editing. Roadmap issue #806 was inaccessible in the Explore sandbox; retry when permitted.

## Scope
- Make instance-selectable tool registration own selector schema and parsing, preserving omission and exact case-sensitive "local" as local. Reads, including Coordinator SQL, accept explicit null locally; send and the watch family reject null. Non-Coordinator SQL remains local-only, and Authority acquires no selector.
- Use a private-construction, non-Deserialize ResolvedPeer for remote API boundaries. Migrate the five selectable tools (send, read conversation, resolve reference, read Goal, query database) and watch/unwatch/list so the old raw-string remote calls no longer compile. Preserve #1029 exact selector matching: the core instance_identity parser trims and cannot replace exact tool selection unmodified.
- Preserve saved-peer selection, credential/TLS/identity/admission checks, immutable remote targets, and no local fallback. Keep local watches distinct from durable remote subscriptions. Extend Federation's normative watch selector requirements explicitly; REQ-FED-001 currently names only the five other tools.
- Coordinate edits to api/federation.rs and api/federation/subscriptions.rs with Federation owner @conv:76f8afbb-549e-48a4-9cfe-83656889afda. Work first in nonoverlapping Coordinator files. Sole host execution/deployment owner @conv:5d57fcc6-e336-469f-850b-63cc253ea0ea must admit any native builds, checks, restart and deployment under ESS free-space constraints; do not duplicate the ongoing DEV Federation check.

## Acceptance
- Registration-derived tests enumerate production Coordinator selector tools, policy/schema, omission/exact-local, valid UUID/name, invalid/ambiguous/malformed inputs, null policies, and no local effects on remote failure, including the watch family. Preserve ordinary/research registry boundaries.
- Compile-fail/privacy tests where practical prevent unchecked resolved-peer construction, Deserialize, and raw selector/ID remote API calls. Add focused ordinary integration regressions for watches and remote failures without duplicating complete subscription lifecycle tests.
- Obtain independent review and resource-admitted relevant tests/CI, then merge and qualify delivery through the designated owners. Report verified results and honestly distinguish compile-time routing guarantees from runtime identity, auth/TLS, admission, and durability guarantees.

## Non-goals
No universal federation or mutation framework, excessive typestate, enrollment/trust changes, unrelated ordinary-agent Goal self-creation, watch-lifecycle redesign, or independent heavy builds/restarts.

Starting points: crates/phoenix-ide/src/coordinator_tools/instance.rs, coordinator_tools.rs, coordinator_tools/goals.rs, api/federation.rs, api/federation/subscriptions.rs, runtime.rs; specs/federation/requirements.md and specs/remote-query-database/requirements.md.
