# Repository Inspection Requirements

## User story

As a Coordinator or authorized Restricted conversation, I want bounded, read-only evidence from an explicitly selected repository scope so that I can compare branches and commits without acquiring Work authority or trusting model-authored shell commands.

## REQ-RI-001: Explicit capability and authority

WHEN repository inspection is offered
THE SYSTEM SHALL expose it as a distinct read-only capability
AND SHALL NOT grant Bash, patch, tmux, process-control, or Work authority as a consequence.

WHEN Coordinator requests inspection
THE SYSTEM SHALL require an explicit persisted `work_scope_id` target.

WHEN a Restricted conversation requests inspection
THE SYSTEM SHALL permit only its own persisted WorkScope target.

## REQ-RI-002: Authoritative target resolution

WHEN resolving a target
THE SYSTEM SHALL read the persisted active WorkScope and its normalized environment
AND SHALL reject missing, retired, Coordinator, global, and repository-less scopes
AND SHALL resolve the repository root from that authoritative identity rather than from a model-supplied path or the caller cwd.

## REQ-RI-003: Structured operations

THE SYSTEM SHALL support structured `resolve_target`, `status`, `log`, `diff`, `read_file`, and `search` operations.

THE SYSTEM SHALL validate refs, paths, limits, and operation-specific fields before execution
AND SHALL NOT accept command strings, executable names, environment variables, shell syntax, redirects, pipes, or arbitrary argv.

## REQ-RI-004: Non-mutation boundary

WHEN executing an inspection
THE SYSTEM SHALL use only allowlisted read-only Git operations with structured argv
AND SHALL disable pagers, aliases, hooks, external diff/text-conversion programs, credential prompts, and network access.

THE SYSTEM SHALL NOT mutate filesystem content, Git refs, the index, worktrees, configuration, or process state.

## REQ-RI-005: Bounded execution and rooted paths

THE SYSTEM SHALL bound execution time, captured output, file bytes, search results, and log results.

WHEN a path is supplied
THE SYSTEM SHALL require a repository-relative path without parent traversal
AND SHALL inspect committed content by resolved Git object identity
AND SHALL canonicalize worktree paths used by worktree-only operations so symlinks cannot escape the authoritative repository root.

## REQ-RI-006: Evidence identity

WHEN an operation completes
THE SYSTEM SHALL return the persisted WorkScope id, canonical repository root, operation, resolved commit identity where relevant, exit status, truncation state, and bounded evidence.

WHEN evidence names source content
THE SYSTEM SHALL include stable `commit:path:line` locations where applicable.

## REQ-RI-007: Compatibility triage

WHEN comparing two branches or commits
THE SYSTEM SHALL allow bounded log, name overlap, diff, committed file reads, and search evidence sufficient to assess whether one change depends on or semantically conflicts with another.

## REQ-RI-008: Network separation

Repository inspection SHALL NOT fetch refs or access GitHub or other network services.
Read-only pull-request metadata and checks require a separate explicit network capability.
