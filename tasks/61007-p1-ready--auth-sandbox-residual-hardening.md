Residual security hardening after the prod unauth-RCE fix (task 61001). Lower severity than the criticals but real on a network-reachable deployment.

- Read handlers accept arbitrary absolute paths: list_directory (handlers.rs ~3656), list_files (~3815), read_file (~3903), validate_cwd (~3613) read/enumerate any caller-supplied path with no root restriction (read_file returns any non-binary file <=10MB). Behind auth, but with no password they give full filesystem recon. mkdir already restricts writes to HOME/tmp — mirror that confinement on reads (same canonicalize + starts_with(root) pattern now used in serve_preview_file).
- Auth cookie value IS the password (auth.rs ~182): no derived/rotatable session token, no Secure flag (cleartext over plain HTTP), and /api/auth/login has no rate-limit/lockout (brute-forceable). Fix: issue a random session token on login, set Secure under TLS, throttle login attempts.
- prod.db created under default umask (phoenix-db lib.rs ~461), possibly 0644 — world-readable on a multi-user host; conversation history holds command output/secrets. Fix: chmod db + -wal/-shm to 0600 or parent dir 0700.
- Explore (read-only) sub-agents get full write/execute bash: for_subagent_explore pushes BashTool (phoenix-tools lib.rs ~597) with an explicit TODO; breaks the Explore read-only invariant (REQ-BASH-008). Fix: land read-only/sandboxed bash or drop BashTool from the explore sub-agent registry until it exists.

Found in spiritual-core audit 2026-06-10.
