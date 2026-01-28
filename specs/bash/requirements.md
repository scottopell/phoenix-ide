# Bash Tool

## User Story

As an LLM agent, I need to execute shell commands reliably so that I can interact with the file system, run builds, manage processes, and accomplish user tasks.

## Requirements

### REQ-BASH-001: Command Execution

WHEN agent requests command execution
THE SYSTEM SHALL execute the command via `bash -c` in the conversation's working directory
AND return combined stdout/stderr output

WHEN command produces output exceeding 128KB
THE SYSTEM SHALL truncate the middle, preserving first and last 4KB
AND indicate truncation occurred

**Rationale:** Agents need reliable command execution with predictable output handling to accomplish file system and build tasks.

---

### REQ-BASH-002: Timeout Management

WHEN agent executes command without `slow_ok` flag
THE SYSTEM SHALL apply 30-second timeout

WHEN agent executes command with `slow_ok=true`
THE SYSTEM SHALL apply 15-minute timeout

WHEN command exceeds its timeout
THE SYSTEM SHALL terminate the process group
AND return partial output with timeout indication

**Rationale:** Agents need appropriate timeouts to prevent hanging on unresponsive commands while allowing longer operations when explicitly expected.

---

### REQ-BASH-003: Background Execution

WHEN agent requests background execution
THE SYSTEM SHALL start the command detached from the agent's lifecycle
AND return immediately with process ID and output file path

WHEN background process completes
THE SYSTEM SHALL append completion status to output file

WHEN agent needs to stop background process
THE SYSTEM SHALL provide process group ID for termination via `kill -9 -<pgid>`

**Rationale:** Agents need to run long-lived processes like servers and demos without blocking conversation flow.

---

### REQ-BASH-004: Process Isolation

WHEN command is executed
THE SYSTEM SHALL create a new process group
AND isolate environment variables containing secrets

WHEN command is terminated (timeout or cancellation)
THE SYSTEM SHALL kill the entire process group
AND wait for cleanup with 15-second grace period

**Rationale:** Agents need clean process management to prevent orphaned processes and secret leakage.

---

### REQ-BASH-005: Working Directory Validation

WHEN command execution is requested
THE SYSTEM SHALL verify working directory exists

WHEN working directory does not exist
THE SYSTEM SHALL return error indicating invalid directory
AND suggest using valid directory

**Rationale:** Agents need clear feedback when operating in invalid directories to recover gracefully.

---

### REQ-BASH-006: Command Safety Check

WHEN command is received
THE SYSTEM SHALL perform basic safety validation
AND reject obviously dangerous patterns

WHEN command fails safety check
THE SYSTEM SHALL return descriptive error
AND not execute the command

**Rationale:** Agents benefit from guardrails that prevent accidental destructive operations.

---

### REQ-BASH-007: Git Co-authorship

WHEN command contains git commit
THE SYSTEM SHALL add co-author trailer for the agent

**Rationale:** Users benefit from clear attribution of AI-assisted commits in version history.

---

### REQ-BASH-008: Interactive Command Handling

WHEN command would invoke interactive editor
THE SYSTEM SHALL configure environment to fail gracefully

WHEN git interactive rebase is attempted in foreground
THE SYSTEM SHALL suggest running as background task

**Rationale:** Agents cannot interact with editors; clear failure messages help agents adapt their approach.

---

### REQ-BASH-009: Tool Schema

WHEN LLM requests bash tool
THE SYSTEM SHALL provide schema with:
- `command` (required string): The shell command to execute
- `slow_ok` (optional boolean): Whether to use extended timeout
- `background` (optional boolean): Whether to run detached

WHEN schema is provided to LLM
THE SYSTEM SHALL include current working directory in description

**Rationale:** Agents need clear, well-documented tool interface to use bash effectively.

---

### REQ-BASH-010: Error Reporting

WHEN command fails with non-zero exit code
THE SYSTEM SHALL include exit code in response
AND include command output for debugging

WHEN command fails due to system error
THE SYSTEM SHALL return descriptive error message
AND distinguish from command execution failure

**Rationale:** Agents need detailed error information to diagnose and recover from failures.
