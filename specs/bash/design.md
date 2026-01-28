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

## Testing Strategy

### Unit Tests
- Output truncation at various sizes
- Timeout behavior (mocked time)
- Mode parsing and validation

### Integration Tests
- Foreground command execution (default and slow modes)
- Background process lifecycle
- Exit code handling

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
├── mod.rs
├── bash/
│   ├── mod.rs
│   ├── executor.rs      # Command execution logic
│   ├── output.rs        # Output formatting/truncation
│   ├── background.rs    # Background execution
│   └── tests.rs
├── patch/
└── think/
```
