# Bash Tool - Design Document

## Overview

The bash tool enables LLM agents to execute shell commands. It is the most critical tool for agent productivity, handling everything from file operations to builds to process management.

## Tool Interface (REQ-BASH-009)

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
    "slow_ok": {
      "type": "boolean",
      "description": "Use extended 15-minute timeout for builds, tests, installs"
    },
    "background": {
      "type": "boolean", 
      "description": "Run detached, return immediately with PID and output file"
    }
  }
}
```

### Description Template

The tool description includes dynamic working directory:

```
Executes shell commands via bash -c, returning combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

With background=true, returns immediately with output redirected to a file.
Use background for servers/demos that need to stay running.

MUST set slow_ok=true for potentially slow commands: builds, downloads,
installs, tests, or any other substantive operation.

IMPORTANT: Keep commands concise. The command input must be less than 60k tokens.
For complex scripts, write them to a file first and then execute the file.

<pwd>{working_directory}</pwd>
```

## Execution Flow (REQ-BASH-001, REQ-BASH-002, REQ-BASH-003)

```rust
impl BashTool {
    pub async fn run(&self, input: BashInput) -> ToolResult {
        // REQ-BASH-005: Validate working directory
        self.validate_working_dir()?;
        
        // REQ-BASH-006: Safety check
        self.check_command_safety(&input.command)?;
        
        // REQ-BASH-007: Add git co-author if needed
        let command = self.add_coauthor_trailer(&input.command);
        
        // Route to appropriate executor
        if input.background {
            self.execute_background(command).await
        } else {
            self.execute_foreground(command, input.slow_ok).await
        }
    }
}
```

## Foreground Execution (REQ-BASH-001, REQ-BASH-002)

```rust
async fn execute_foreground(&self, command: String, slow_ok: bool) -> ToolResult {
    let timeout = if slow_ok {
        Duration::from_secs(15 * 60)  // 15 minutes
    } else {
        Duration::from_secs(30)        // 30 seconds
    };
    
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &command])
       .current_dir(&self.working_dir)
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());
    
    // REQ-BASH-004: Process group isolation
    cmd.process_group(0);  // Create new process group
    
    // REQ-BASH-004: Environment isolation
    cmd.env_clear();
    for (key, value) in std::env::vars() {
        if !self.is_secret_env(&key) {
            cmd.env(key, value);
        }
    }
    cmd.env("EDITOR", "/bin/false");  // REQ-BASH-008
    
    let child = cmd.spawn()?;
    
    match tokio::time::timeout(timeout, self.wait_with_output(child)).await {
        Ok(result) => self.format_output(result),
        Err(_) => {
            // REQ-BASH-002: Kill process group on timeout
            self.kill_process_group(child.id());
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
    // Create temp directory for output
    let tmp_dir = tempfile::tempdir()?;
    let output_file = tmp_dir.path().join("output");
    let output_handle = File::create(&output_file)?;
    
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &command])
       .current_dir(&self.working_dir)
       .stdin(Stdio::null())
       .stdout(output_handle.try_clone()?)
       .stderr(output_handle)
       .process_group(0);  // REQ-BASH-004
    
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

## Output Formatting (REQ-BASH-001)

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
    
    // REQ-BASH-010: Include exit code for failures
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

## Safety Checks (REQ-BASH-006)

Basic pattern-based safety validation:

```rust
fn check_command_safety(&self, command: &str) -> Result<(), ToolError> {
    // Block obviously dangerous patterns
    let dangerous_patterns = [
        "rm -rf /",
        "rm -rf /*",
        "> /dev/sda",
        "mkfs.",
        "dd if=.* of=/dev/",
    ];
    
    for pattern in &dangerous_patterns {
        if command.contains(pattern) {
            return Err(ToolError::SafetyViolation(format!(
                "Command contains dangerous pattern: {}", pattern
            )));
        }
    }
    
    Ok(())
}
```

Note: This is defense-in-depth, not a security boundary. The primary security model relies on the LLM's training and user oversight.

## Git Co-authorship (REQ-BASH-007)

```rust
fn add_coauthor_trailer(&self, command: &str) -> String {
    // Detect git commit commands and add co-author
    if self.is_git_commit(command) {
        let trailer = "Co-authored-by: Phoenix <phoenix@exe.dev>";
        // Use git's trailer mechanism or append to message
        self.inject_coauthor(command, trailer)
    } else {
        command.to_string()
    }
}
```

## Interactive Command Handling (REQ-BASH-008)

Environment configuration prevents interactive editors from blocking:

```rust
// Set in environment for all commands
cmd.env("EDITOR", "/bin/false");
cmd.env("VISUAL", "/bin/false");

// Special handling for git interactive rebase
if !input.background && command.contains("git rebase -i") {
    cmd.env("GIT_SEQUENCE_EDITOR", 
        "echo 'Interactive rebase requires background=true' && exit 1");
}
```

## Process Group Management (REQ-BASH-004)

```rust
fn kill_process_group(&self, pid: u32) {
    // Kill entire process group (negative PID)
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

// Environment variables to filter (secrets)
fn is_secret_env(&self, key: &str) -> bool {
    let secret_prefixes = [
        "ANTHROPIC_",
        "OPENAI_",
        "GEMINI_",
        "AWS_SECRET",
        "PHOENIX_",  // Internal Phoenix secrets
    ];
    secret_prefixes.iter().any(|p| key.starts_with(p))
}
```

## Working Directory Validation (REQ-BASH-005)

```rust
fn validate_working_dir(&self) -> Result<(), ToolError> {
    let path = Path::new(&self.working_dir);
    
    if !path.exists() {
        return Err(ToolError::InvalidWorkingDir(format!(
            "Working directory does not exist: {} (conversation may need to be recreated)",
            self.working_dir
        )));
    }
    
    if !path.is_dir() {
        return Err(ToolError::InvalidWorkingDir(format!(
            "Working directory is not a directory: {}",
            self.working_dir
        )));
    }
    
    Ok(())
}
```

## Testing Strategy

### Unit Tests
- Output truncation at various sizes
- Timeout behavior (mocked time)
- Safety check patterns
- Git co-author injection
- Environment filtering

### Integration Tests
- Foreground command execution
- Background process lifecycle
- Process group cleanup on timeout
- Working directory validation

### Property Tests
```rust
#[proptest]
fn output_never_exceeds_limit(output: String) {
    let formatted = format_output(output);
    assert!(formatted.len() <= MAX_OUTPUT_LENGTH + 200);  // Allow for metadata
}

#[proptest]
fn secrets_never_leaked(env_vars: HashMap<String, String>) {
    let filtered = filter_environment(&env_vars);
    for (key, _) in &filtered {
        assert!(!is_secret_env(key));
    }
}
```

## File Organization

```
src/tools/
├── mod.rs
├── bash/
│   ├── mod.rs
│   ├── executor.rs      # Command execution logic
│   ├── safety.rs        # Safety checks
│   ├── output.rs        # Output formatting/truncation
│   ├── background.rs    # Background execution
│   └── tests.rs
├── patch/
└── think/
```
