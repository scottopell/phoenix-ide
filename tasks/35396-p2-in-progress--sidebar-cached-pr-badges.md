# Add cached PR badges to conversation sidebar

Show a small PR badge in each conversation row when Phoenix already knows that the conversation's work scope is associated with a pull request.

## Goal

Users can scan the conversation sidebar and see which Work/Branch conversations already have an open/known PR, without opening each conversation.

## Scope

- Treat PR identity as work-scope data, not conversation-only data.
- Add a cached PR summary to the conversation list data from existing `work_scope_pr_associations` records.
- Render a compact badge in `ConversationRow` beside the existing mode badge.
- Reuse the existing PR badge label/style helpers where practical, but sidebar MVP must not show check status.
- Do not add sidebar `gh` calls.
- Do not poll PR status from the sidebar.
- Do not fan out `/pr-status` calls per conversation.

## MVP behavior

For conversations whose work scope has a cached primary PR association:

- show `#123` for open PRs
- show `#123 draft` for draft PRs
- show `#123 merged` for merged PRs
- show `#123 closed` for closed PRs
- link badge to the PR URL
- tooltip includes PR title and branch/base if available

If no cached PR association exists, show no badge.

## Efficiency rule

Sidebar rendering is cache-only. The active conversation's existing PR status refresh may continue to warm/update the cache, but merely viewing the sidebar must cost zero `gh` calls.

## Suggested implementation

1. Add a lightweight cached PR summary type for conversation list rows, derived from `primary_work_scope_pr_association(work_scope)`.
2. Populate it while building conversation list responses, deduping by `work_scope_key` so continuations sharing a worktree reuse one DB lookup/result.
3. Add matching TypeScript shape in `Conversation`.
4. Add `SidebarPrBadge` or equivalent small component.
5. Update `ConversationList` tests for:
   - no badge when no cached PR
   - badge shown for cached open/draft/merged/closed states
   - two conversations with same work scope can show same cached PR without extra API behavior
6. Add backend tests that conversation list PR summaries come from DB cache only, not `gh`.

## Non-goals

- No checks badge in sidebar.
- No freshness badge in sidebar.
- No background PR refresh job.
- No hover-to-refresh in this task.
