# Think Tool - Design Document

## Overview

The think tool is the simplest tool in the system. It provides LLMs a dedicated space for reasoning, planning, and note-taking without any external effects.

## Tool Interface (REQ-THINK-002)

### Schema

```json
{
  "type": "object",
  "required": ["thoughts"],
  "properties": {
    "thoughts": {
      "type": "string",
      "description": "The thoughts, notes, or plans to record"
    }
  }
}
```

### Description

```
Think out loud, take notes, form plans. Has no external effects.
```

## Implementation (REQ-THINK-001)

```rust
struct ThinkInput {
    thoughts: String,
}

impl ThinkTool {
    pub fn run(&self, input: ThinkInput) -> ToolResult {
        // No side effects - just acknowledge
        ToolResult::Success("recorded".to_string())
    }
}
```

## Testing Strategy

### Unit Tests
- Tool always returns "recorded"
- No state changes occur
- Any string input is accepted

## File Organization

```
src/tools/
├── think.rs          # Single file, minimal implementation
```
