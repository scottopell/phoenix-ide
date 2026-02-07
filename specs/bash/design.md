# Bash Tool - Design Document

## Overview

The bash tool enables LLM agents to execute shell commands. It is the most critical tool for agent productivity, handling everything from file operations to builds to process management.

## Tool Interface (REQ-BASH-006)

### Schema

```json
{
  "type": "object",
  "required": ["command"],
  "properties": {
    "command": {
      "type": "string",
      "description": "Shell command to execute via bash -c"
    },
    "mode": {
      "type": "string",
      "enum": ["default", "slow", "background"],
      "description": "Execution mode: default (30s timeout), slow (15min timeout), background (detached)"
    }
  }
}
```

The single `mode` enum replaces separate `slow_ok` and `background` booleans, making invalid state combinations unrepresentable.

### Mode Semantics

| Mode | Timeout | Behavior |
|------|---------|----------|
| `default` (or omitted) | 30 seconds | Foreground, blocks until complete |
| `slow` | 15 minutes | Foreground, for builds/tests/installs |
| `background` | 24 hours | Detached, returns immediately with PID |

### Description Template

The tool description includes dynamic working directory:

```
Executes shell commands via bash -c, returning combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

With mode="background", returns immediately with output redirected to a file.
Use background for servers/demos that need to stay running.

Use mode="slow" for potentially slow commands: builds, downloads,
installs, tests, or any other substantive operation.

IMPORTANT: Keep commands concise. The command input must be less than 60k tokens.
For complex scripts, write them to a file first and then execute the file.

<pwd>{working_directory}</pwd>
```

## Execution Flow (REQ-BASH-001, REQ-BASH-002, REQ-BASH-003)

```rust
#[derive(Debug, Clone, Copy, Default)]
enum ExecutionMode {
    #[default]
    Default,
    Slow,
    Background,
}

struct BashInput {
    command: String,
    mode: ExecutionMode,
}

impl BashTool {
    pub async fn run(&self, input: BashInput) -> ToolResult {
        match input.mode {
            ExecutionMode::Background => self.execute_background(input.command).await,
            mode => self.execute_foreground(input.command, mode).await,
        }
    }
}
```

## Foreground Execution (REQ-BASH-001, REQ-BASH-002, REQ-BASH-004)

```rust
async fn execute_foreground(&self, command: String, mode: ExecutionMode) -> ToolResult {
    let timeout = match mode {
        ExecutionMode::Default => Duration::from_secs(30),
        ExecutionMode::Slow => Duration::from_secs(15 * 60),
        ExecutionMode::Background => unreachable!(),
    };
    
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &command])
       .current_dir(&self.working_dir)
       .stdin(Stdio::null())       // REQ-BASH-004: No TTY
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());
    
    let child = cmd.spawn()?;
    
    match tokio::time::timeout(timeout, self.wait_with_output(child)).await {
        Ok(result) => self.format_output(result),
        Err(_) => {
            // REQ-BASH-002: Kill process on timeout
            child.kill();
            ToolResult::Error(format!(
                "[command timed out after {:?}]", timeout
            ))
        }
    }
}
```

## Background Execution (REQ-BASH-003)

```rust
async fn execute_background(&self, command: String) -> ToolResult {
    let timeout = Duration::from_secs(24 * 60 * 60);  // 24 hours
    
    // Create temp directory for output
    let tmp_dir = tempfile::tempdir()?;
    let output_file = tmp_dir.path().join("output");
    let output_handle = File::create(&output_file)?;
    
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &command])
       .current_dir(&self.working_dir)
       .stdin(Stdio::null())
       .stdout(output_handle.try_clone()?)
       .stderr(output_handle);
    
    let child = cmd.spawn()?;
    let pid = child.id();
    
    // Spawn task to monitor completion
    let output_path = output_file.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        let mut file = OpenOptions::new().append(true).open(&output_path)?;
        match status {
            Ok(s) if s.success() => {
                writeln!(file, "\n\n[background process completed]")?;
            }
            Ok(s) => {
                writeln!(file, "\n\n[background process failed: {}]", s)?;
            }
            Err(e) => {
                writeln!(file, "\n\n[background process error: {}]", e)?;
            }
        }
        Ok::<_, std::io::Error>(())
    });
    
    // Return immediately with process info
    ToolResult::Success(format!(
        "<pid>{}</pid>\n<output_file>{}</output_file>\n<reminder>To stop: kill -9 -{}</reminder>",
        pid, output_file.display(), pid
    ))
}
```

## Output Formatting (REQ-BASH-001, REQ-BASH-006)

```rust
const MAX_OUTPUT_LENGTH: usize = 128 * 1024;  // 128KB
const SNIP_SIZE: usize = 4 * 1024;            // 4KB each end

fn format_output(&self, result: CommandResult) -> ToolResult {
    let output = result.stdout_stderr_combined();
    
    let formatted = if output.len() > MAX_OUTPUT_LENGTH {
        format!(
            "[output truncated in middle: got {}, max is {}]\n{}\n\n[snip]\n\n{}",
            humanize_bytes(output.len()),
            humanize_bytes(MAX_OUTPUT_LENGTH),
            &output[..SNIP_SIZE],
            &output[output.len() - SNIP_SIZE..]
        )
    } else {
        output
    };
    
    // REQ-BASH-006: Include exit code for failures
    if !result.status.success() {
        ToolResult::Error(format!(
            "[command failed: exit code {}]\n{}",
            result.status.code().unwrap_or(-1),
            formatted
        ))
    } else {
        ToolResult::Success(formatted)
    }
}
```

## Command Safety Checks (REQ-BASH-007)

Before execution, commands are parsed and checked for dangerous patterns.

### Architecture

```rust
// src/tools/bash_check.rs

pub fn check(script: &str) -> Result<(), CheckError> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let tree = parser.parse(script, None)?;
    check_node(tree.root_node(), script.as_bytes())
}
```

### Check Functions

| Check | Blocked Patterns | Allowed |
|-------|-----------------|----------|
| `no_blind_git_add` | `git add -A`, `git add .`, `git add --all`, `git add *` | `git add file.rs` |
| `no_force_push` | `git push --force`, `git push -f` | `git push --force-with-lease` |
| `no_dangerous_rm` | `rm -rf /`, `rm -rf ~`, `rm -rf .git`, `rm -rf *` | `rm -rf node_modules` |

### Sudo Handling

The `sudo` prefix is stripped before checking:
```rust
let args = if args.first() == Some(&"sudo".to_string()) {
    &args[1..]
} else {
    &args[..]
};
```

### Pipeline/Compound Command Handling

The parser walks the full AST, checking each command node:
```rust
fn check_node(node: Node, source: &[u8]) -> Result<(), CheckError> {
    if node.kind() == "command" {
        check_command(node, source)?;
    }
    // Recurse into children (handles &&, ||, pipes, etc.)
    for child in node.children(&mut cursor) {
        check_node(child, source)?;
    }
    Ok(())
}
```

### Integration Point

```rust
// In BashTool::run()
async fn run(&self, input: Value, cancel: CancellationToken) -> ToolOutput {
    // ... parse input ...
    
    // REQ-BASH-007: Check for dangerous patterns
    if let Err(e) = bash_check::check(&input.command) {
        return ToolOutput::error(e.message);
    }
    
    // ... execute command ...
}
```

### Error Messages

Error messages are descriptive and suggest alternatives:

```
permission denied: blind git add commands (git add -A, git add ., 
git add --all, git add *) are not allowed, specify files explicitly
```

```
permission denied: git push --force is not allowed. Use 
--force-with-lease for safer force pushes, or push without force
```

```
permission denied: this rm command could delete critical data 
(.git, home directory, or root). Specify the full path explicitly 
(no wildcards, ~, or $HOME)
```

---

## Testing Strategy

### Unit Tests
- Output truncation at various sizes
- Timeout behavior (mocked time)
- Mode parsing and validation

### Integration Tests
- Foreground command execution (default and slow modes)
- Background process lifecycle
- Exit code handling

### Command Safety Check Tests (REQ-BASH-007)

42 unit tests covering:
- Git add patterns (allowed and blocked)
- Git push patterns (force vs force-with-lease)
- Rm patterns (dangerous paths vs safe paths)
- Sudo prefix handling
- Pipeline/compound commands
- Edge cases (empty scripts, comments)

4 integration tests verifying checks run before execution:
- `test_blocked_git_add`
- `test_blocked_rm_rf_root`
- `test_blocked_git_push_force`
- `test_allowed_command_runs`

### Property Tests
```rust
#[proptest]
fn output_never_exceeds_limit(output: String) {
    let formatted = format_output(output);
    assert!(formatted.len() <= MAX_OUTPUT_LENGTH + 200);  // Allow for metadata
}
```

## File Organization

```
src/tools/
├── mod.rs               # Tool registry, trait definitions
├── bash.rs              # BashTool implementation (REQ-BASH-001 through 006)
├── bash_check.rs        # Command safety checks (REQ-BASH-007)
├── patch.rs             # PatchTool
├── patch/               # Patch tool internals
├── think.rs             # ThinkTool
├── keyword_search.rs    # KeywordSearchTool
├── read_image.rs        # ReadImageTool
└── subagent.rs          # Sub-agent tools
```
