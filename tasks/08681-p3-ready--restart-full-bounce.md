---
created: 2026-04-24
priority: p3
status: ready
artifact: dev.py
---

./dev.py restart currently only bounces Phoenix — Vite is left running. Discovered this causes real bugs: a long-running Vite process against upgraded node_modules (e.g. after vite 5→8 upgrade) serves a mis-patched /@vite/client that errors with "__BUNDLED_DEV__ is not defined" and the whole UI goes blank. The fix required ./dev.py down && ./dev.py up, which is what the user expected restart to do in the first place.

Change restart to perform a full down + up by default. Rationale: restart is "bounce the stack to ensure good state" — a half-restart that leaves a stale daemon running undermines the whole point. If there is a fast-path for Rust-only changes worth preserving, expose it as a separate flag (e.g. ./dev.py restart --rust-only) rather than the default.

Verify: after change, running ./dev.py restart should stop and restart BOTH Phoenix and Vite. The restart output should clearly indicate which processes were bounced.
