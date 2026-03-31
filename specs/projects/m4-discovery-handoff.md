# M4 Spec Discovery Handoff

## Target
specs/projects/ (update design.md, requirements.md) + tasks/0604

## Scope
update-requirements + update-design: M4 section rewrite. Significant departure from original spec.

## Motivation
Work conversations accumulate with no completion path. Worktrees bloat disk. The user needs a clean exit from Work mode that handles code landing and cleanup. The original spec over-engineered this with AwaitingMergeApproval state, diff review, and circular Work->Explore transitions. The real need is simpler: land the code, clean up, and close the conversation.

## Personas
Single persona: the developer using Phoenix. Two project contexts:
1. **Solo dev**: merges directly into base branch. No PR needed.
2. **Team collaborator**: needs a PR against a base branch (possibly not main -- e.g., a shared feature branch). Phoenix assists but doesn't create the PR.

Both personas share the same cleanup needs (worktree deletion, task file updates, conversation termination).

## User Journeys

### Journey 1: Complete (squash merge)
Trigger: User clicks "Complete" button on an idle Work conversation.
1. Phoenix pre-checks: no uncommitted changes in worktree, no merge conflicts with base_branch
2. If task file not in "done" status, nudge user to ask agent to update it
3. LLM generates semantic commit message from `git diff base_branch...HEAD` (spinner shown)
4. Editable commit message shown in confirmation dialog
5. User confirms -> squash merge into base_branch, delete worktree+branch, conversation -> Terminal
Expected outcome: code landed on base branch as single commit, worktree cleaned up, conversation closed.

### Journey 2: Abandon (destructive discard)
Trigger: User clicks "Abandon" button on an idle Work conversation.
1. Confirmation dialog warns: permanent deletion of all work
2. User confirms -> delete worktree+branch, commit task file status change to wont-do on base_branch, conversation -> Terminal
Expected outcome: worktree gone, branch gone, task marked wont-do, conversation closed.

### Journey 3: Awareness of base branch advancement
Trigger: Passive, on SSE connect + periodic poll (~60s).
1. Phoenix checks if base_branch has new commits since the worktree branched
2. Shows "N behind" badge in StateBar next to branch name
3. Updates dynamically (agent can rebase, badge reflects new state on next poll)
No action taken automatically. User can ask agent to rebase if needed.

## Critical Paths
- **Complete with conflicts**: Must fail cleanly with actionable message. User asks agent to rebase, then retries.
- **Complete with dirty worktree**: Must block, not silently lose uncommitted work.
- **Abandon confirmation**: Must require explicit confirmation. Destructive and irreversible.

## Key Design Decisions (departures from original spec)

1. **Terminal, not circular**: Work conversations go to Terminal state after complete/abandon. No "return to Explore mode." This eliminates state machine complexity around mode transitions. User creates a new Explore conversation if needed.

2. **No AwaitingMergeApproval state**: User-initiated only (button click). No agent-initiated review tool. No diff display step. Assumes the user reviewed work live during the conversation.

3. **Squash merge only**: Other merge strategies available manually outside Phoenix. Keeps the UX simple and the commit history clean.

4. **LLM-generated commit message**: Based on the final diff, following semantic commit style (concept-focused, no file names). Editable before confirm.

5. **base_branch stored in ConvMode::Work**: Recorded at approval time from the checked-out branch of the Explore conversation. Supports feature-branch workflows (not just main).

6. **Commits-behind is passive**: Poll-based, no filesystem watcher. No rebase automation. The agent already has bash access to run `git rebase` when asked.

7. **Idle-only actions**: Complete/Abandon buttons disabled while agent is working. User cancels first if needed.

## Edge Cases
- Merge conflicts: fail with error, user asks agent to rebase
- Dirty worktree: block with error
- Agent active: buttons disabled, cancel first
- base_branch deleted upstream: fail with error at merge time
- Task file status not "done" at complete time: nudge (non-blocking)
- Worktree already deleted (manual cleanup): reconciliation handles this on restart (M3)

## Unknowns
- PR creation assistance: deferred. Phoenix could suggest title+description but doesn't create PRs. May revisit based on demand.
- Rebase automation: explicitly out of scope. Agent has bash access.

## Cross-Spec Impact
- `specs/bedrock/design.md`: AwaitingMergeApproval section should be removed or marked superseded
- `specs/bedrock/requirements.md`: REQ-BED-029 (Return to Explore) changes to "go Terminal"
- `specs/projects/requirements.md`: REQ-PROJ-009 rewritten (no AwaitingMergeApproval state)
- `specs/projects/design.md`: Executor git operations table updated
- `tasks/0604`: acceptance criteria updated to match new design
- `src/db/schema.rs`: ConvMode::Work gets base_branch field
