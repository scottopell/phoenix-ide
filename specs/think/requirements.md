# Think Tool

## User Story

As an LLM agent, I need a way to think out loud, take notes, and form plans without any side effects so that I can organize my reasoning before taking actions.

## Requirements

### REQ-THINK-001: Thought Recording

WHEN agent requests think tool
THE SYSTEM SHALL accept the thoughts parameter
AND return a simple acknowledgment
AND produce no side effects

**Rationale:** LLMs benefit from explicit reasoning steps. Providing a dedicated tool for thinking encourages structured problem-solving without cluttering conversation with tool-use artifacts.

---

### REQ-THINK-002: Tool Schema

WHEN LLM requests think tool
THE SYSTEM SHALL provide schema with:
- `thoughts` (required string): The thoughts, notes, or plans to record

**Rationale:** Simple, single-parameter schema keeps the tool focused on its purpose.
