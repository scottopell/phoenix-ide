# drive-turn Executive Summary

## Current Reality

`drive-turn` is a separate workspace binary that drives one user turn without starting HTTP, SSE, TLS, or UI services. The `phoenix-ide` package exposes its existing server modules as a library; both the server binary and `phoenix-drive-turn` link that same library. The driver constructs the production `RuntimeManager`, provider registry, database adapter, MCP manager, built-in tool registry, state machine, and continuation loop, awaits initial MCP discovery, then observes the runtime's authoritative state watch until a typed stable outcome is also visible in persistence.

The CLI supports transient in-memory SQLite, retained unique temporary-file SQLite, and an explicit retained database path. Successful output is one JSON object containing raw persisted messages and run metadata. A persisted user message distinguishes a completed empty-response idle turn from the conversation's initial idle state. A timed-out turn is cancelled through the production cancellation event and must reach a persisted stable state before the driver returns. Awaiting external recovery is reported as a stable outcome. Every return path tears down conversation-owned tmux, browser, and bash resources.

## Usage

```bash
./dev.py drive-turn \
  --cwd /path/to/fixture \
  --model gpt-5.5 \
  --prompt-file prompt.txt \
  --memory

./dev.py drive-turn \
  --cwd /path/to/fixture \
  --model gpt-5.5 \
  --prompt 'Perform the requested edit.' \
  --temp-db \
  --timeout 180
```

The `dev.py` wrapper layers `.phoenix-ide.env` and `.phoenix-ide.dev.env` exactly as development server startup does, builds the driver, resolves the executable from Cargo's effective target directory, and forwards all arguments. Direct binary invocation remains available when the caller has already prepared the process environment. The process reads the same LLM environment variables as the server. Standard output is reserved for JSON. The CLI installs structured runtime tracing on standard error using `RUST_LOG` when set and the Phoenix development filter otherwise; invocation and runtime failures are also written to standard error with a non-zero exit status.

## Verification

Live smoke tests using `gpt-5.5` completed through both memory and retained temporary-file modes, reached authoritative `Idle`, and persisted user and agent messages. A timed-out live bash turn (`sleep 8; touch marker`) returned after production cancellation settled; the marker remained absent and the retained database stopped changing after return. A natural patch-recovery fixture also produced the prescribed initial multi-patch call, received the real patch error through the production tool path, recovered with normal model behavior, and produced the expected file.

A preliminary control/treatment review compared production patch code before and after request-position diagnostics while holding the driver and prompt constant. All 18 observed runs conformed to their required initial tool call, produced the correct final file, and succeeded on the next patch attempt. In the repeated duplicate-anchor cohort, both arms performed 2 reads/searches and 8 total recovery calls across 6 runs. Missing-anchor behavior also matched: both arms re-read before retrying. The review is therefore neutral rather than evidence of a behavioral improvement; it validates the evaluation path and shows that matching-location diagnostics already let `gpt-5.5` infer many duplicate failures without an explicit request position.

## Status

| Requirement | Status | Evidence |
|---|---|---|
| REQ-DRIVE-TURN-001 | Complete | Server and driver link the same `phoenix_ide` library and `RuntimeManager` |
| REQ-DRIVE-TURN-002 | Complete | Typed stable outcomes, authoritative watch, timeout |
| REQ-DRIVE-TURN-003 | Complete | Memory, temporary-file, and explicit-file modes |
| REQ-DRIVE-TURN-004 | Complete | Serialized raw run result and persisted messages |
| REQ-DRIVE-TURN-005 | Complete | Production working-directory validation and Direct conversation scope |
