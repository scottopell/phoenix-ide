---
created: 2026-05-25
priority: p1
status: ready
artifact: phoenix-client.py
---

phoenix-client.py fails against any HTTPS prod URL (e.g. https://essmbp.local:8031) with "Error: cannot connect to Phoenix server. Is it running? (./dev.py up)" even though the server is up and responding (curl -ks works fine).

httpx.Client() in PhoenixClient.__init__ uses default cert verification — the prod TLS cert (essmbp.local self-signed / local CA) isn't in the system trust store the script's python env sees. The catch-all in main() then misreports the TLS failure as "cannot connect", hiding the real error.

Repro:
  PHOENIX_API_URL=https://essmbp.local:8031 ./phoenix-client.py --list-models

Expected: model list.
Actual: misleading "cannot connect" error.

Fixes worth considering:
- Honor an env var (PHOENIX_TLS_INSECURE=1 → verify=False), or auto-detect *.local hosts.
- Surface the actual httpx exception in the error path instead of collapsing every connect-class failure into the same message.
- Document how to point the script at the local CA bundle.

Discovered while debugging codex quota event emission (couldn't drive prod from the client; had to fall back to curl).
