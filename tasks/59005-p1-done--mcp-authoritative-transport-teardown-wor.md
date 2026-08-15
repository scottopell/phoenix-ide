# MCP Authoritative Transport Teardown Worker

Conceptually replaces the uncommitted task 44018 from another branch without depending on that task file or worktree.

Implement standalone MCP-local authoritative transport teardown. Make transport shutdown truthful and retryable; retain failed supervisor and manager handles for terminal teardown retry; preserve primary failure causes alongside teardown failures; and propagate reload/OAuth reconfiguration failures while always cleaning up OAuth listener state.

Scope is limited to MCP transport, supervisor, reload/OAuth completion, and manager shutdown retention. It excludes manager-wide admission/lifecycle redesign, server or request draining, drive-turn orchestration, and Repository Cutover.
