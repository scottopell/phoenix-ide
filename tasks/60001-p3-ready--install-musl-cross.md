---
created: 2026-05-03
priority: p3
status: ready
artifact: dev.py
---

Build x86_64-linux-musl cross-compiler from source via musl-cross-make so `cargo check --target x86_64-unknown-linux-musl` works on macOS without Homebrew.

Context:
- `./dev.py check` runs a musl smoke check on darwin (dev.py:928-931) to catch breakage before `./dev.py prod deploy`.
- This needs `x86_64-linux-musl-gcc` on PATH. The standard install path is `brew install FiloSottile/musl-cross/musl-cross`, but this machine uses MacPorts as the primary package manager and does not own /opt/homebrew.
- MacPorts has `x86_64-linux-binutils` but no musl gcc port, so MacPorts alone cannot satisfy the toolchain.
- Note: macOS `prod deploy` itself uses target=None (native ARM, see dev.py:2108), so this is purely about restoring the smoke check, not unblocking deploys.

Recipe:
1. Clone https://github.com/richfelker/musl-cross-make
2. `make TARGET=x86_64-linux-musl`  (~30-60min compile)
3. `make TARGET=x86_64-linux-musl install OUTPUT=$HOME/local/musl-cross`
4. Add `$HOME/local/musl-cross/bin` to PATH (shell profile)
5. Verify: `x86_64-linux-musl-gcc --version` and re-run `./dev.py check`

Until this is done, the musl smoke check is auto-skipped via the toolchain-detection hack added to dev.py.
