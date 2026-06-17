REQ-TPANEL-008 ("Don't Silently Take Over a Terminal I Have Open Elsewhere") is marked in the spec as NOT implemented. The current reclaim path (REQ-TERM-003, task 24691) auto-reclaims: a second WebSocket connection for an already-attached scope sends StopReason::Detach to the sitting relay and takes over the PTY. The evicted TerminalPanel sees a plain WS close and renders the generic "Shell exited" + reconnect affordance, which on click steals the session back — a two-tab tug-of-war.

This is most visible with the global terminal now reachable from both /new and /terminal, but it affects any scope opened in two tabs.

Fix per REQ-TPANEL-008: the backend must signal a reclaim/"attached elsewhere" close distinctly from a shell exit (e.g. a dedicated WS close code or control frame before Detach), and the frontend must render an explicit "attached in another tab — Reclaim" state instead of "Shell exited", without auto-reclaiming. This flips the UX from silent auto-reclaim toward consent-based reclaim and touches both the terminal WS protocol and specs/terminal + specs/terminal-panel.

Surfaced by Codex review on the /terminal PR.
