# Narrow CommandPalette action dependencies

Task 51001 identified candidate C10 as plausible but too broad for the current pass: `CommandPalette` action construction depends on the full conversations array, so list polling/SSE metadata changes may rebuild actions and rebind effects even when visible commands are unchanged.

Evidence to gather:
- Keep the command palette closed while conversation polling/SSE updates active metadata.
- Open the palette and count action rebuilds, shortcut listener registrations, and search effect restarts.

Acceptance criteria:
- Establish baseline raw samples/counts before any code changes.
- If validated, narrow action-shape dependencies to primitives and read latest conversations from a ref at execution time where correctness requires latest data.
- Preserve archive-current and navigation correctness.
