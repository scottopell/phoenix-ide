# Make About host-local disk actions safe and explicit

Depends on the shared access presentation from the identity/access task.

## User outcome

Disk inspection, file-manager reveal, conversation navigation, and leftover-worktree cleanup are understandable and safe from both local and remote browsers.

## Scope

- Rename ambiguous `Reveal` affordances to describe the server-host file-manager action precisely.
- Show disabled/explanatory remote states instead of relying only on hidden buttons.
- Add explicit confirmation for destructive leftover-worktree cleanup, including path and disposition.
- Report cleanup success and failure in the affected row without silently replacing context.
- Preserve typed backend revalidation immediately before mutation and the distinction between live worktrees and cleanup-eligible leftovers.
- Audit path displays so server filesystem locations are never presented as client-resolvable links.
- Add local/remote, live/leftover, success/failure, and stale-revalidation tests.

## Acceptance criteria

- [ ] Remote users understand that paths belong to the Phoenix server host.
- [ ] Reveal communicates that it opens the containing folder on that host.
- [ ] Cleanup cannot execute without explicit confirmation.
- [ ] A worktree that becomes live before confirmation is rejected safely.
- [ ] Success/failure feedback remains associated with the exact row acted upon.
