Phoenix spawns tmux servers (sockets under ~/.phoenix-ide/tmux-sockets/) for its
terminal tool. These self-daemonize (double-fork into their own session), so they
are NOT in Phoenix's process group and survive Phoenix shutdown. When a worktree
or conversation is torn down without an explicit tmux kill, the server is orphaned
indefinitely. A May 2026 sweep found 84 orphaned tmux servers accumulated this way.

Contrast: MCP servers (playwright/chrome-devtools/mcp-remote) spawned by Phoenix
ARE in its process group and die when Phoenix is killed — so they self-resolve.
tmux is the lone exception because it deliberately detaches.

Note: dev tooling (./dev.py reap) now cleans orphaned *dev servers* (Phoenix/Vite)
whose worktree was deleted, but deliberately does NOT touch tmux — tmux lifecycle
is an app concern, not a dev-tooling concern.

Why this is nuanced (resolve before implementing):
- tmux session lifecycle is tied to a conversation, and sometimes to a conversation
  *chain* — a child conversation may legitimately reuse / attach to a session
  started by its parent. Killing on single-conversation teardown could break a chain.
- Detached-but-alive is sometimes intentional (user wants to reattach later).
- Need to decide the ownership/GC model: ref-count per chain? kill on chain
  terminal-state? startup reconciliation that kills servers whose owning
  conversation no longer exists in the DB? TTL on idle detached servers?
- Stale socket files in ~/.phoenix-ide/tmux-sockets/ also linger after a server
  dies — decide whether GC removes those too.

Deliverable: a design decision (likely a short spEARS spec or Allium @guidance on
the terminal/tmux module) for when Phoenix tears down tmux servers it owns, then
implement. Startup reconciliation (kill servers with no live owning conversation)
is the likely floor; chain-aware lifecycle is the open question.
