# Commission Review Requirements

## User need

Agents need a Phoenix-native way to commission an independent review of the active work without relying on external review CLIs or separate provider credentials.

## Requirements

### REQ-CR-001 Phoenix LLM provider stack

Commission review shall use Phoenix's configured LLM selection and shall not require external provider credentials.

### REQ-CR-002 Mandatory capital brief

Commission review shall require a non-empty `brief` that explains why review is useful at this point in the work.

### REQ-CR-003 Human approval gate

Commission review execution shall be gated by human approval because it can spend a significant token budget.

### REQ-CR-004 Inferred review target

Commission review shall infer the review target from conversation and worktree context rather than requiring agents to provide refs, commits, or diff plumbing.

### REQ-CR-005 Dirty worktree opt-in

A git-aware task or worktree review shall require a clean worktree unless `allow_dirty_working_tree` is explicitly true.

### REQ-CR-006 Dirty state disclosure

When a dirty review is allowed, the approval and result surfaces shall disclose the dirty state and explicit opt-in.

### REQ-CR-007 Structurally unavailable unsupported states

Unsupported states shall not expose `commission_review`; invalid review states shall not be representable as ordinary successful tool calls.

### REQ-CR-008 Read-only operation

Commission review shall not edit files, stage changes, commit, push, or move refs.

### REQ-CR-009 Cancellation

Long-running commission review work shall honor the conversation cancellation token.

### REQ-CR-010 Skipped-file warnings

Large or unsupported files shall be reported as warnings and shall not be silently ignored.
