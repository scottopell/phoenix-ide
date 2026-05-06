---
created: 2026-05-05
priority: p3
status: in-progress
artifact: src/main.rs
---

Add HTTP/2 + TLS to phoenix's HTTP server so a single browser-to-phoenix
TCP connection can multiplex an unbounded number of concurrent streams,
eliminating the 6-connections-per-origin saturation class of bugs
structurally.

## Motivating example

2026-05-05 incident: clicking "Approve task" appeared to hang the entire
app — POST queued in the browser indefinitely, no log entry on phoenix.
Diagnosis: browser had ~6 SSE streams open across conversation tabs,
saturating its HTTP/1.1 per-origin connection pool. The Approve POST
queued behind them in the browser's connection scheduler and never
dispatched.

Bounded SSE lifetime (task 02704) makes this self-healing on a 30-minute
cadence. HTTP/2 makes it impossible: stream multiplexing means there is
no per-origin connection ceiling to saturate.

## Why TLS is required (the gotcha)

Browsers refuse h2c (HTTP/2 cleartext). HTTP/2 from a browser is only
spoken over TLS (ALPN-negotiated). So this task is really "add TLS,
which unlocks HTTP/2." There is no plain-HTTP path to HTTP/2 in any
production browser.

Phoenix's deployment is direct (no fronting reverse proxy in the path),
so TLS termination happens in the phoenix binary itself.

## Scope

- Axum/hyper TLS+ALPN config wired into `src/main.rs`
- Cert + key loaded from configured paths; ALPN advertises h2 + http/1.1
- HTTP/1.1 path remains supported for any non-browser client (curl,
  phoenix-client.py) that doesn't negotiate h2
- Cert source: resolved as opt-in auto-managed private Phoenix CA for
  single-user/internal-DNS deployments, with manual cert/key paths for
  environments that already manage TLS externally.
- Cert rotation story: `PHOENIX_TLS=auto` reuses the managed CA and rotates
  the server leaf certificate on startup. For remote production hosts,
  `./dev.py tls issue <host>` creates a host-specific bundle from the local CA
  and `./dev.py tls install <bundle>` configures `PHOENIX_TLS=manual` without
  copying the CA private key to the remote host.

## Done When

- [x] Phoenix serves HTTPS on the same port (or a configurable port)
- [ ] Browser hits `https://...:8031` and negotiates h2 via ALPN
- [x] Multiple SSE streams + concurrent fetches succeed past the
      historical 6-connection threshold (manual repro: open 8+ tabs
      and verify a 9th request still completes)
- [x] HTTP/1.1 clients (curl without `--http2`) still work
- [x] Cert + key path configurable via env / dev.py
- [x] Cert source decision documented (self-signed vs CA-issued)
- [x] `./dev.py up --https` and `./dev.py tls ca|issue|install` workflows exist

## Out of scope

- Public ACME management. Phoenix's target use here is single-user/internal
  DNS. Public CA issuance may be appropriate for public DNS zones, but is not
  Phoenix's built-in path.
- Reverse proxy alternative (Caddy etc.) — adds moving parts; not worth it
  for a single-binary tool. Direct termination is simpler.

## Notes

Filed 2026-05-05 as the structural follow-up to tasks 02708
(shutdown-sse-deadline) and 02704 (bounded-sse-stream-lifetime). Do
02704 first; only escalate to this task if saturation persists past
the lifetime fix, since the cert-management overhead is real.

Automated verification:

- `curl -k https://127.0.0.1:8033/version` returns `HTTP/2 200`.
- `curl -k --http1.1 https://127.0.0.1:8033/version` returns `HTTP/1.1 200 OK`.
- `openssl s_client -alpn h2` negotiates `ALPN protocol: h2`.
- Node `http2` smoke test opened 8 concurrent
  `/api/conversations/:id/stream` SSE streams over one h2 session, then
  completed a ninth `/version` request in 3 ms while all 8 SSE streams stayed
  active.

Browser verification is intentionally still unchecked: without trusting
`~/.phoenix-ide/tls/phoenix-local-ca.pem`, the in-app browser stops at
`ERR_CERT_AUTHORITY_INVALID`. That is the expected trust boundary, not a server
failure.
