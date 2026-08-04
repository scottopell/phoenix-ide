# Complete native ownership coverage for thermal policy

The macOS thermal observe-only governor currently reports only WorkScope-owned Bash process groups as authoritative targets. Add typed PID/start/process-group ownership for conversation terminals, Browser/Chromium trees, and local stdio MCP infrastructure without guessing by command line, cwd, or profile path.

Browser must capture native identity at the actual chromiumoxide launch boundary and unify explicit, idle, cascade, and shutdown cleanup. Local stdio MCP children must receive dedicated process groups and group-wide lifecycle cleanup; remote HTTP MCP remains structurally excluded. Terminal sampling must retain its WorkScope owner. Update resource attribution and thermal coverage only after PID-reuse-safe identity is authoritative.

Acceptance: no unrelated process can become eligible; unsupported Chrome/tmux topology is explicit; lifecycle tests prove descendant cleanup; all capability gaps are logged and surfaced.
