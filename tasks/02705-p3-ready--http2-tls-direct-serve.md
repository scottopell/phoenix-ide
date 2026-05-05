---
created: 2026-05-05
priority: p3
status: ready
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
- Cert source: open question. Options range from a long-lived
  self-signed cert (acceptable for a single-user dev tool with
  trust-on-first-use) to whatever CA is appropriate for the
  deployment environment
- Cert rotation story: if self-signed, document the manual procedure;
  if CA-issued, document the renewal path

## Done When

- [ ] Phoenix serves HTTPS on the same port (or a configurable port)
- [ ] Browser hits `https://...:8031` and negotiates h2 via ALPN
- [ ] Multiple SSE streams + concurrent fetches succeed past the
      historical 6-connection threshold (manual repro: open 8+ tabs
      and verify a 9th request still completes)
- [ ] HTTP/1.1 clients (curl without `--http2`) still work
- [ ] Cert + key path configurable via env / dev.py
- [ ] Cert source decision documented (self-signed vs CA-issued)

## Out of scope

- Auto-cert management (ACME/Let's Encrypt) — phoenix is direct-internal,
  not internet-facing, so ACME http-01 is not viable. Manual cert
  installation is the baseline.
- Reverse proxy alternative (Caddy etc.) — adds moving parts; not
  worth it for a single-binary tool. Direct termination is simpler.

## Notes

Filed 2026-05-05 as the structural follow-up to tasks 02703
(shutdown-sse-deadline) and 02704 (bounded-sse-stream-lifetime). Do
02704 first; only escalate to this task if saturation persists past
the lifetime fix, since the cert-management overhead is real.
