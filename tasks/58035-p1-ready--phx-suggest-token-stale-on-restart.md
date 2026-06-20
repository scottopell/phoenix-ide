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

No fully-graceful auto-fix exists for an already-running pane: its shell already
exported its PATH, so it cannot pick up `phx` without a new window or
`exec $SHELL`. But the two halves of the staleness differ in fixability, and an
empirical check settled how much a live refresh recovers:

- Hyperlinks (OSC-8 forwarding): FULLY fixable live. `tmux set -ag
  terminal-features ",*:hyperlinks"` on a reused server restores forwarding for
  the next fresh attach (verified: pre-feature server + live set -ag + fresh
  attach forwards the phxrun link, identical to a config-loaded server). Phoenix
  re-attaches on every panel open, so the user gets it on next open.
- `phx` on PATH + injected env: NOT fixable for the current pane (frozen shell
  env). `set-environment -g` reaches only NEW windows/panes.

Resolved design (non-destructive refresh + a one-time hint):

1. Detection: stamp the server's global env with a companion version
   (`set-environment -g PHOENIX_COMPANION_VERSION <v>`) whenever it is created or
   refreshed. On `ensure_live` reuse, read it back (`show-environment -g`); a
   missing/older stamp means stale.

2. Refresh-on-reuse (gated on a stale stamp, so a current server pays nothing):
   - `set -ag terminal-features ",*:hyperlinks"` — restores OSC-8 forwarding.
   - `set -g default-command` to a wrapper that prepends the `phx` bin dir to
     PATH before exec-ing the shell, so new windows/panes resolve `phx`.
     (Implementation note, verified empirically: `set-environment -g PATH` is
     silently ignored by tmux for new panes — they take PATH from the server
     process — so it CANNOT inject phx; `default-command` is the seam that does.
     Non-PATH vars below DO propagate via the global env.)
   - `set-environment -g` PHOENIX_API_URL / PHOENIX_SUGGEST_TOKEN.
   - update the version stamp.

3. Current-pane UX: print a one-time hint into the stale pane, e.g. "phx is now
   available — open a new window or run `exec $SHELL` to use it in this shell."
   Guides the user across the one-time gap without touching their running shell.

Explicitly rejected: kill+recreate of a live server (destroys running panes/
processes) — unnecessary, since the live refresh fixes hyperlinks and new
windows fully.

Out of scope (already done): token/API-URL staleness — the token is persisted
in app_settings and fingerprint-bound; `PHOENIX_API_URL` is derived from the
bind address.
