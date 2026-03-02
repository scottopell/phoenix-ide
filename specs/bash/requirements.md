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

WHEN agent executes command in default mode
THE SYSTEM SHALL apply 30-second timeout

WHEN agent executes command in slow mode
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

### REQ-BASH-004: No TTY Attached

WHEN command is executed
THE SYSTEM SHALL run without a TTY attached

**Rationale:** Agents cannot interact with terminal prompts; commands must operate in non-interactive mode.

---

### REQ-BASH-005: Tool Schema

WHEN LLM requests bash tool
THE SYSTEM SHALL provide schema with:
- `command` (required string): The shell command to execute
- `mode` (optional enum): Execution mode - `default`, `slow`, or `background`

WHEN mode is omitted
THE SYSTEM SHALL use default mode (30-second foreground)

WHEN schema is provided to LLM
THE SYSTEM SHALL include current working directory in description

**Rationale:** Agents need clear, well-documented tool interface. Single mode enum prevents invalid state combinations.

---

### REQ-BASH-006: Error Reporting

WHEN command fails with non-zero exit code
THE SYSTEM SHALL include exit code in response
AND include command output for debugging

WHEN command fails due to system error
THE SYSTEM SHALL return descriptive error message
AND distinguish from command execution failure

**Rationale:** Agents need detailed error information to diagnose and recover from failures.

---

### REQ-BASH-007: Command Safety Checks

WHEN bash command is submitted for execution
THE SYSTEM SHALL parse the command using a shell syntax parser (tree-sitter-bash)
AND check for dangerous patterns before execution

WHEN dangerous pattern is detected
THE SYSTEM SHALL reject the command with descriptive error message
AND NOT execute the command

THE SYSTEM SHALL reject the following patterns:
- Blind git add: `git add -A`, `git add .`, `git add --all`, `git add *`
- Force push: `git push --force`, `git push -f` (allow `--force-with-lease`)
- Dangerous rm: `rm -rf` on `/`, `~`, `$HOME`, `.git`, `*`, `.*`

WHEN command has `sudo` prefix
THE SYSTEM SHALL apply safety checks to the command following sudo

WHEN command appears in a pipeline or compound command
THE SYSTEM SHALL check all command components

**Rationale:** LLMs sometimes execute dangerous commands despite instructions. Parsing-based checks provide UX guardrails with helpful error messages. This is NOT a security boundary - just catches common mistakes and guides toward safer alternatives.

---

### REQ-BASH-008: Landlock Enforcement for Explore Mode

WHEN conversation is in Explore mode AND Landlock is available (Linux 5.13+)
THE SYSTEM SHALL execute bash commands under Landlock restrictions providing:
- Read-only filesystem access (all writes blocked at kernel level)
- No outbound network (TCP connect/bind blocked)
- Signal scoping (kernel 6.12+): processes cannot signal outside sandbox
- Resource limits via rlimits (memory, CPU time, process count)

WHEN Landlock blocks an operation in Explore mode
THE SYSTEM SHALL return the kernel error (EACCES, EPERM)
AND the tool description SHALL include a clear explanation of sandbox constraints

WHEN conversation is in Work mode
THE SYSTEM SHALL NOT apply Landlock restrictions
AND bash commands SHALL have write access within the conversation's worktree directory

**Rationale:** Landlock is defense-in-depth for Explore mode read-only enforcement on
Linux. It is NOT the primary isolation mechanism for the overall system — that role
belongs to git worktrees (see `specs/projects/`), which provide physical directory
isolation on all platforms. Landlock adds kernel-level enforcement preventing bypass
of Explore mode constraints via clever shell commands or prompt injection on supported
Linux kernels.

> **Landlock Feature Matrix:**
> | Kernel | ABI | Features |
> |--------|-----|----------|
> | 6.12+  | v6  | Full protection: filesystem, network, ioctl, signal/socket scoping |
> | 6.10-6.11 | v5 | + Device ioctl blocking |
> | 6.7-6.9 | v4 | Filesystem + network (TCP) |
> | 5.13-6.6 | v1-v3 | Filesystem only |
>
> Recommended: Kernel 6.12+ for full signal scoping; 6.7+ minimum for network blocking.

---

### REQ-BASH-009: Graceful Degradation Without Landlock

WHEN Landlock is unavailable (non-Linux OS or Linux kernel < 5.13)
THE SYSTEM SHALL detect this at startup
AND log a warning that Explore mode read-only enforcement is advisory only
AND continue to enforce read-only semantics at the application layer

WHEN running on a non-Linux operating system
THE SYSTEM SHALL enforce Explore mode read-only constraints at the tool level only
AND indicate to users that kernel-level enforcement is unavailable
AND note that physical worktree isolation (REQ-PROJ-005) still provides write-write
isolation between conversations on all platforms

WHEN degraded mode is active
THE SYSTEM SHALL still apply command safety checks (REQ-BASH-007)
AND the absence of Landlock SHALL NOT prevent Work mode from functioning

**Rationale:** Worktree isolation works on all platforms — two Work conversations
cannot touch each other's files regardless of OS because they operate in separate
physical directories. Landlock's absence only weakens the read-only guarantee for
Explore mode conversations (an advisory constraint becomes advisory without kernel
backing). This is a meaningful but bounded reduction in protection, not a failure
of the overall isolation model.

---

### REQ-BASH-010: Stateless Tool with Context Injection

WHEN bash tool is invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext` parameter
AND derive working directory from `ToolContext.working_dir`
AND use `ToolContext.cancel` for cancellation handling

WHEN bash tool is constructed
THE SYSTEM SHALL NOT store per-conversation state
AND tool instance SHALL be reusable across conversations

**Rationale:** Stateless tools with context injection eliminate the possibility of using stale or incorrect conversation state. All context flows through a single, validated parameter created at call time.

---

### REQ-BASH-011: Display Command Simplification

WHEN bash tool result is displayed in the UI
THE SYSTEM SHALL simplify the command for display by removing boilerplate prefixes
AND provide a `display` field alongside the original `command`

WHEN command contains `cd <path> && <rest>`
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display only `<rest>` (strip the redundant cd)

WHEN command contains `cd <path> && <rest>`
AND `<path>` does NOT match the conversation's working directory
THE SYSTEM SHALL display the full command unchanged

WHEN command contains `cd <path>; <rest>` (semicolon separator)
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display only `<rest>`

WHEN command contains `||` (or operator)
THE SYSTEM SHALL preserve the full command including fallback
AND NOT strip any prefix before `||`

WHEN command contains mixed operators like `cd <path> && cmd || fallback`
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display `cmd || fallback` (strip only the matching cd)

**Rationale:** LLMs often emit `cd /path && actual_command` patterns. Stripping redundant cd prefixes reduces visual noise in the UI. However, stripping cd when it changes to a *different* directory would be misleading—users need to see that the command ran elsewhere. The `||` operator indicates error handling that users should see; stripping the primary command would hide important context about what was attempted.
