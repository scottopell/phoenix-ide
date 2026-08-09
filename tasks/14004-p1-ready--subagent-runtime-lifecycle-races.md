# Fix supported-journey subagent runtime lifecycle races

PR #635 exposed three correctness bugs that are independently reachable on `main`; they do not require parallel Work subagents.

## Confirmed bugs

1. **Fresh spawn versus reconstruction:** after the child row exists but before the fresh route is published, opening the child's live stream can call `RuntimeManager::get_or_create` and materialize a second runtime. Fresh creation and reconstruction do not share one owner or completion.
2. **Reconstructed child loses parent result routing:** `RuntimeManager::materialize_runtime` recognizes a persisted subagent but does not restore its parent event route. A resumed/live-viewed child can reach `Effect::NotifyParent` with no parent sender, dropping the real terminal outcome while the parent remains pending.
3. **Cancellation head-of-line blocking:** the global subagent handler awaits full spawn materialization in the same serial loop that consumes cancellation requests. A slow spawn can delay cancellation of an already-installed child, including across unrelated parent conversations or among supported parallel Explore children.

## Scope and design requirements

- Fix these current-main bugs independently of parallel Work admission.
- Preserve the one-Work-child gate.
- Do not transplant PR #635's six-state `SubAgentControl`; exact-head review showed it became an incomplete child workflow coordinator.
- Establish one typed materialization owner/join boundary for fresh creation and reconstruction.
- Restore parent routing for reconstructed children without making the runtime map a lifecycle authority.
- Make installed-child cancellation prompt without blocking behind spawn materialization.
- Define terminal delivery ownership explicitly rather than retaining a payloadless pending marker.
- Coordinate with ProductConversation lifecycle work; do not destabilize or delay the P0 program.

## Required regressions

- Opening one child during fresh spawn produces one runtime and one initial task injection.
- A reconstructed child reports its terminal outcome to its persisted parent.
- Cancelling an installed child is not delayed by an unrelated blocked/slow spawn.
- Existing parallel Explore and sequential Work journeys remain valid.
