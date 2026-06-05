Serve the Vite dev server over HTTPS so the browser↔dev hop negotiates HTTP/2 (Vite 8 uses http2.createSecureServer with allowHTTP1 whenever server.https is set — the old proxy→HTTP/1 fallback is gone). This lifts the browser per-host connection cap in dev, matching the TLS/h2 prod deployment, so multiple concurrent sub-agent SSE streams no longer compete for the ~6 HTTP/1.1 connection budget.

Approach: reuse Phoenix's existing auto local-CA leaf (PHOENIX_TLS=auto issues ~/.phoenix-ide/tls/{phoenix-local-ca.pem, phoenix-local-server.pem, key}). dev.py: enable auto-TLS by default, pass the cert/key paths to Vite via env, point the /api proxy at the https Phoenix upstream, and print the UI URL as https. vite.config: set server.https from VITE_HTTPS_CERT/VITE_HTTPS_KEY env when present. One-time: trust the local CA in the browser/OS.

Context: follow-up to PR #219, which removed the inline-stream single-live-stream cap that existed to protect the HTTP/1.1 connection budget in dev.
