# Bash Tool - Executive Summary

## Requirements Summary

The bash tool enables LLM agents to execute shell commands in the conversation's working directory. Commands run via `bash -c` with combined stdout/stderr output, no TTY attached. Three execution modes exist: default (30-second timeout), slow (15-minute timeout for builds/tests), and background (detached, returns immediately with PID). The single mode enum prevents invalid state combinations. Output exceeding 128KB is truncated in the middle. Detailed error reporting includes exit codes and distinguishes command failures from system errors.

## Technical Summary

Implemented as a stateless Tool receiving all context via `ToolContext` parameter. The tool schema defines `command` (required) and `mode` (optional enum: default/slow/background). Foreground execution uses tokio process spawning with mode-dependent timeouts and cancellation support via `ctx.cancel`. Background execution detaches the process, redirects output to a temp file, and spawns a monitoring task. Working directory comes from `ctx.working_dir`, not stored state. Output truncation preserves 4KB from each end when exceeding 128KB limit. No TTY is attached; stdin is null.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BASH-001:** Command Execution | ✅ Complete | bash -c execution with working dir |
| **REQ-BASH-002:** Timeout Management | ✅ Complete | 30s default, 15min slow mode |
| **REQ-BASH-003:** Background Execution | ✅ Complete | Detached with output file, returns PID |
| **REQ-BASH-004:** No TTY Attached | ✅ Complete | stdin null, process group |
| **REQ-BASH-005:** Tool Schema | ✅ Complete | Schema with mode enum (default/slow/background) |
| **REQ-BASH-006:** Error Reporting | ✅ Complete | Exit codes, truncated output |
| **REQ-BASH-007:** Command Safety Checks | ✅ Complete | tree-sitter parsing, pattern rejection |
| **REQ-BASH-008:** Landlock Enforcement | ❌ Not Started | Read-only fs, no network in Restricted mode |
| **REQ-BASH-009:** Graceful Degradation | ❌ Not Started | Non-Linux and old kernel fallback |
| **REQ-BASH-010:** Stateless Tool with Context | 🔄 In Progress | ToolContext refactor |

**Progress:** 7 of 10 complete
