# Simple Client

## User Story

As an LLM agent or developer, I need a simple command-line client to interact with the Phoenix API so that I can test and use the system without requiring the full React web UI.

## Requirements

### REQ-CLI-001: Single-Shot Execution

WHEN user runs the client with a message
THE SYSTEM SHALL send the message to the conversation
AND poll for completion
AND print the agent's response
AND exit

WHEN agent response includes tool use and results
THE SYSTEM SHALL display tool names, inputs, and outputs in readable format

**Rationale:** LLM agents work best with discrete command invocations rather than interactive sessions. Single-shot execution fits naturally into agent tool use patterns.

---

### REQ-CLI-002: Conversation Management

WHEN user specifies conversation ID or slug
THE SYSTEM SHALL continue that existing conversation

WHEN user specifies working directory without conversation
THE SYSTEM SHALL create a new conversation in that directory

WHEN neither is specified
THE SYSTEM SHALL use current working directory for new conversation

**Rationale:** Agents need to both start new conversations and continue existing ones.

---

### REQ-CLI-003: Image Support

WHEN user provides image file paths
THE SYSTEM SHALL read and base64-encode the images
AND include them in the message payload

WHEN image file cannot be read
THE SYSTEM SHALL exit with error before sending message

**Rationale:** Agents need to share screenshots and diagrams with Phoenix.

---

### REQ-CLI-004: Output Format

WHEN displaying agent response
THE SYSTEM SHALL format output for LLM comprehension:
- Clear section delimiters for message boundaries
- Tool use blocks with name and input
- Tool result blocks with output
- Final text response clearly marked

WHEN displaying errors
THE SYSTEM SHALL print to stderr with clear error indication

**Rationale:** Output must be easily parsed by LLM agents reading the response.

---

### REQ-CLI-005: Polling Behavior

WHEN waiting for agent completion
THE SYSTEM SHALL poll the conversation endpoint at reasonable interval
AND continue until conversation state is idle or error

WHEN conversation enters error state
THE SYSTEM SHALL display error message and exit with non-zero code

WHEN polling times out (configurable, default 10 minutes)
THE SYSTEM SHALL exit with timeout error

**Rationale:** Simple polling avoids SSE complexity while still providing completion detection.

---

### REQ-CLI-006: Configuration

WHEN API endpoint is needed
THE SYSTEM SHALL check in order:
1. `--api-url` command-line flag
2. `PHOENIX_API_URL` environment variable
3. Default to `http://localhost:8000`

WHEN conversation is specified
THE SYSTEM SHALL accept either:
- `--conversation` or `-c` flag with ID or slug
- `PHOENIX_CONVERSATION` environment variable

**Rationale:** Environment variables enable persistent configuration; flags enable per-invocation override.

---

### REQ-CLI-007: Single File Distribution

WHEN client is distributed
THE SYSTEM SHALL be a single Python file
AND use PEP 723 inline script metadata for dependencies
AND be runnable via `uv run client.py`

**Rationale:** Single file with inline deps maximizes portability and simplifies distribution.
