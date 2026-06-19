Pre-merge gate for the `phx` command-suggestion feature.

Problem: on startup Phoenix mints a fresh PHOENIX_SUGGEST_TOKEN and injects it
(plus PHOENIX_API_URL) into every PTY's environment. tmux servers persist
across a Phoenix restart, and their pane shells captured the OLD token at
pane-creation time. After a restart the in-terminal `phx` therefore sends a
stale token and POST /api/suggest returns 403 until the tmux server happens to
be recreated. PHOENIX_API_URL has the same staleness exposure if the bind port
changes across restarts.

Repro:
1. Start Phoenix, open a terminal (spawns a tmux server), confirm `phx "..."`
   works.
2. Restart Phoenix (mints a new token).
3. In the SAME terminal, run `phx "..."` -> 403 invalid/missing suggest token.

Root cause: the token is session-minted (per-process) but the env carrying it
lives in a longer-lived tmux server that outlives the process that minted it.
Two lifetimes that should match do not.

Handle gracefully (decide + implement before merge):
- Preferred: persist the suggest token across restarts (data_dir/DB) instead of
  minting a new one each start. It is stable capability config, not a session
  secret that needs rotation, so a stable value makes existing panes keep
  working with zero extra machinery. Rotate only on an explicit secret-rotation
  trigger (e.g. password change), mirroring SessionStore's fingerprint binding.
- Alternative: on startup, refresh live tmux servers via `tmux set-environment
  -g` for the scope (note: already-exported vars in existing panes won't update
  without re-export, so this is partial).
- Alternative: accept a short grace set of recently-valid tokens.

Also covers: the same persist-vs-mint decision should make PHOENIX_API_URL
robust to a port change across restarts, or document that the port is stable.

---

Token staleness is resolved: the token is persisted in app_settings and bound
to the password fingerprint, so a restart keeps it stable (existing terminals
stay authorized) and a password change re-mints it.

Remaining (broader staleness of a pre-feature tmux server): the env/PATH/config
injection runs only when spawn_session CREATES a server. When ensure_live finds
an already-live server (created before this feature, or before an upgrade), it
reuses it, so its panes lack `phx` on PATH and the OSC-8 hyperlinks
terminal-feature until the server is recreated. This only affects terminals
left open across the upgrade; a newly-opened terminal is fine.

No fully-graceful auto-fix exists: an already-running pane's shell has already
exported its PATH, so `tmux set-environment -g` / `set -ag` on the live server
only reach NEW windows/panes, and kill+recreate would destroy the user's
running panes and processes. Options to weigh:
- Non-destructive partial refresh on reuse (set-environment -g + set -ag),
  helping new windows only.
- Detect a pre-feature server and recreate it behind explicit per-session
  consent (destructive otherwise).
- Surface a one-click "restart this terminal's tmux server" affordance in the
  UI so the user opts in.
