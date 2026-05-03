---
created: 2026-05-03
priority: p3
status: done
artifact: dev.py
---

Restore the macOS musl smoke check (`cargo check --target x86_64-unknown-linux-musl`)
that `./dev.py check` runs at dev.py:928-940 by providing an `x86_64-linux-musl-gcc`
on PATH.

Context:
- `./dev.py check` runs a musl smoke check on darwin to catch breakage before
  `./dev.py prod deploy`. Auto-skipped if `x86_64-linux-musl-gcc` isn't on PATH.
- macOS `prod deploy` itself uses target=None (native ARM, see dev.py:2108), so
  this is purely a static-analysis canary, not a deploy blocker.
- This machine uses MacPorts as its primary package manager and does not own
  /opt/homebrew, so the standard `brew install FiloSottile/musl-cross/musl-cross`
  path doesn't apply. MacPorts has `x86_64-linux-binutils` but no musl gcc port.

Resolution (zig-shim approach, what's actually installed):

This routes the smoke check through zig's bundled clang+lld, which already
ships a working cross-toolchain for x86_64-linux-musl. No hour-long gcc
bootstrap, ~5 minutes total install.

1. `sudo port install zig`  (MacPorts has zig in category `lang`)
2. `cargo install cargo-zigbuild`  (not strictly needed if using shims, but
   useful for `cargo zigbuild` invocations elsewhere)
3. Drop these three shims in a directory that's on PATH (e.g. `~/.local/bin`):

   `x86_64-linux-musl-gcc`:
   ```bash
   #!/bin/bash
   # Forward to zig's clang, target x86_64-linux-musl.
   # Strip cc-rs's --target=<rust-triple> because it uses Rust's 4-segment
   # format (e.g. x86_64-unknown-linux-musl) which zig can't parse — zig
   # expects 3-segment (x86_64-linux-musl). We already pass -target ourselves.
   filtered=()
   for a; do
     [[ "$a" == --target=* ]] || filtered+=("$a")
   done
   exec zig cc -target x86_64-linux-musl "${filtered[@]}"
   ```

   `x86_64-linux-musl-ar`:
   ```sh
   #!/bin/sh
   exec zig ar "$@"
   ```

   `x86_64-linux-musl-ranlib`:
   ```sh
   #!/bin/sh
   exec zig ranlib "$@"
   ```

4. `chmod +x` all three.
5. Verify: `x86_64-linux-musl-gcc --version` (should report clang via zig),
   then `cargo check --target x86_64-unknown-linux-musl` from the repo root.
6. Re-run `./dev.py check` and confirm the `cargo check musl` step runs
   instead of being skipped.

Why three shims: cc-rs (used by `*-sys` build scripts like libsqlite3-sys and
zstd-sys) probes for the compiler by name, then invokes ar to bundle compiled
C objects into a static lib that rustc links. Both must exist on PATH under
the expected `<triple>-<tool>` names. Ranlib included for safety; cc-rs may
not always invoke it but no harm.

Why the gcc shim is more complex than the others: cc-rs appends
`--target=x86_64-unknown-linux-musl` (Rust's 4-segment triple) to every
compiler invocation. Zig parses `--target=` and rejects the 4-segment form,
so we strip it from argv before exec'ing zig. This is essentially what
`cargo-zigbuild` does internally; the shim approach makes it explicit and
keeps `./dev.py check` working unchanged.

Alternative (musl-cross-make from source) — not used here, kept for reference:

1. Clone https://github.com/richfelker/musl-cross-make
2. `make TARGET=x86_64-linux-musl`  (~30-60min compile)
3. `make TARGET=x86_64-linux-musl install OUTPUT=$HOME/local/musl-cross`
4. Add `$HOME/local/musl-cross/bin` to PATH

Produces a real FSF gcc cross-toolchain at a stable path. More heavyweight,
but reusable for non-Rust cross-builds and doesn't depend on zig's release
cadence. Worth doing if zig ever drops or changes its musl support, or if
you need a cross gcc for C/C++ work outside this repo.
