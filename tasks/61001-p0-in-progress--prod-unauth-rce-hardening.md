Prod deploy shipped an unauthenticated remote-code-execution surface plus an unauthenticated arbitrary-file read. Captured for posterity; the fixes land in the same PR as this task.

## C1 — unauthenticated network RCE
`./dev.py prod deploy` composed three facts into unauth RCE as the deploying user:
- systemd socket `ListenStream={port}` binds all interfaces (dev.py generate_systemd_socket); Rust fallback `([0,0,0,0], port)` (main.rs).
- `auth_middleware` bypasses entirely when `PHOENIX_PASSWORD` is unset (api/auth.rs); `generate_systemd_service` never writes a password.
- `create_conversation` accepts any `req.cwd` that exists+is_dir (api/handlers.rs) — bash tool runs there.
Anyone reaching :8031 POSTs /api/conversations/new with cwd:"/" and drives bash.

### Fix landed
- main.rs: fail closed — refuse a non-loopback bind when PHOENIX_PASSWORD is unset, escape hatch `PHOENIX_ALLOW_INSECURE_BIND=1`. Defends regardless of launcher.
- dev.py start_phoenix sets PHOENIX_ALLOW_INSECURE_BIND=1 (dev is loopback-trusted); prod deploy paths deliberately do not.

## C2 — permissive CORS / drive-by RCE
main.rs CORS was `allow_origin(Any)` + no CSRF: any website the user visits could fetch the API.
### Fix landed
- main.rs: permissive CORS only when no password (loopback dev); strict same-origin when a password is set.

## C3 — unauthenticated arbitrary file read via /preview
`serve_preview_file` mapped GET /preview/<path> -> fs::read("/<path>") with no traversal guard, and /preview was auth-exempt. Read /etc/passwd, ~/.ssh, prod.db (<=10MB).
### Fix landed
- handlers.rs: canonicalize + require the resolved path live under some conversation working dir (Database::preview_roots).
- auth.rs: removed the /preview/ auth exemption (same-origin sandboxed iframe carries the cookie).

## Residual (tracked separately, p1)
Read handlers (list_directory/read_file/validate_cwd) still accept arbitrary absolute paths; auth cookie value IS the password with no Secure flag / no login rate-limit; prod.db default umask; Explore-mode sub-agents still get write/execute bash.
