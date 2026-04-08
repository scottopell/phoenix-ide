---
created: 2026-04-08
priority: p2
status: in-progress
artifact: pending
---

# fix-allium-spec-issues

## Plan

## Fix Allium Spec Issues Found During Review

### Summary
The two Allium specs (`bedrock.allium`, `projects.allium`) are production-grade but have a field naming inconsistency and a missing rule for behavior that already exists in code.

### Context
Review of the specs against the implementation revealed:
1. **bedrock.allium** has a field name mismatch — rules reference a field name that doesn't match the declaration
2. **projects.allium** doesn't model worktree reconciliation on server restart, even though the code already does this (`reconcile_worktrees()` in `src/main.rs:205-293`)

### What to do

#### 1. Fix field name references in `specs/bedrock/bedrock.allium`

The spec declares `pending_sub_agents_cancelling` (line 198) for the `cancelling_tool` state, but three rules reference the undeclared name `pending_sub_agents`:
- Line 582: `conversation.pending_sub_agents.count = 0` → should be `conversation.pending_sub_agents_cancelling.count = 0`
- Line 587: `conversation.sub_agents_pending = conversation.pending_sub_agents` → should be `...= conversation.pending_sub_agents_cancelling`
- Line 607: same as 582
- Line 612: same as 587
- Line 622: `conversation.pending_sub_agents.remove(result.agent)` → should be `conversation.pending_sub_agents_cancelling.remove(result.agent)`

**Note:** The Rust code uses `pending_sub_agents` as the field name in both `ToolExecuting` and `CancellingTool` enum variants. The spec intentionally distinguishes these as separate semantic fields (`accumulated_sub_agents` during execution, `pending_sub_agents_cancelling` during cancellation). The rules should match the spec's own declarations, not the code's field names — the spec models semantic roles, not struct layouts.

#### 2. Add worktree reconciliation rule to `specs/projects/projects.allium`

The code in `src/main.rs` (`reconcile_worktrees()`) already handles orphaned worktrees on server restart:
- Scans all Work-mode conversations
- Detects missing worktrees or empty `worktree_path`/`base_branch`
- Reverts affected conversations to Explore mode
- Resets CWD to project root
- Runs `git worktree prune` per unique project root

This behavior should be modeled in the spec. Add a rule (e.g., `ServerRestartWorktreeReconciliation`) in the projects spec that:
- References bedrock's `ServerRestart` event (lines 997-1001 of bedrock.allium)
- Documents the detection conditions (missing dir, empty fields)
- Documents the recovery actions (revert mode, reset CWD, prune)
- Includes a `@guidance` block with the sequence from the implementation

Also verify whether a new requirement ID is needed or if this falls under existing REQ-PROJ coverage.

### Acceptance Criteria
- [ ] All `pending_sub_agents` references in bedrock.allium rules match the declared field name `pending_sub_agents_cancelling`
- [ ] projects.allium contains a rule modeling worktree reconciliation on server restart, matching the behavior in `src/main.rs:reconcile_worktrees()`
- [ ] No new open questions introduced — any ambiguities discovered are resolved inline
- [ ] `./dev.py check` passes (specs are not compiled, but ensure no code changes needed)


## Progress

