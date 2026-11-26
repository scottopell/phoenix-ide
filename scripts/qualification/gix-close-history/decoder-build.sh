#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
probe=$PWD
mkdir -p decoder-build/src
cp decoder.rs decoder-build/src/main.rs
cp Cargo.lock decoder-build/Cargo.lock
cat > decoder-build/Cargo.toml <<'MANIFEST'
[package]
name = "decode-probe"
version = "0.0.0"
edition = "2024"
rust-version = "1.95"

[workspace]

[features]
candidate = ["dep:gix-ref"]
hex-only = ["dep:gix-hash"]

[dependencies]
gix-ref = { version = "=0.70.0", optional = true, default-features = false, features = ["sha1", "sha256"] }
gix-hash = { version = "=0.29.0", optional = true, default-features = false, features = ["sha1", "sha256"] }
MANIFEST
# Extract the exact baseline from this checkout, not a second handwritten parser.
{
    echo '#[allow(dead_code)] struct GitOidIdentity(String);'
    grep '^const GIT_OID_HEX_LEN' "$root/crates/phoenix-core/src/domain/close.rs"
    echo '#[allow(dead_code)] impl GitOidIdentity {'
    echo '#[cfg(not(feature = "hex-only"))]'
    sed -n '/^    pub fn parse_hex(value: impl Into<String>)/,/^    }/p' "$root/crates/phoenix-core/src/domain/close.rs"
    echo '#[cfg(feature = "hex-only")] pub fn parse_hex(value: &str) -> Result<(), impl std::fmt::Display> { gix_hash::ObjectId::from_hex(value.as_bytes()).map(|_| ()) }'
    echo '}'
    sed -n '/^pub enum GitOidIdentityError {/,/^}/p' "$root/crates/phoenix-core/src/domain/close.rs" | sed '/#\[error(/d'
    echo 'impl std::fmt::Display for GitOidIdentityError { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "invalid oid") } }'
    sed -n '/^fn parse_target_head_reflog_roots(/,/^}/p' "$root/crates/phoenix-ide/src/runtime/close_retirement.rs"
} > decoder-build/src/baseline.rs
cd decoder-build
cargo generate-lockfile --offline
for mode in baseline candidate hex-only; do
    flags=()
    if [[ $mode != baseline ]]; then flags+=(--features "$mode"); fi
    cargo tree --offline --locked "${flags[@]}" --prefix none --format '{p}' | sed 's/ (\*)$//' | sort -u > "$probe/decoder-$mode-deps.txt"
    CARGO_TARGET_DIR="target/$mode" /usr/bin/time -f "$mode build_s=%e peak_rss_kib=%M" cargo build --offline --locked --release "${flags[@]}"
    wc -c "target/$mode/release/decode-probe"
done
cp "$probe/head-cli.rs" src/head-cli.rs
sed -n '/^enum InspectedHead {/,/^}/p; /^fn inspect_head_with_leaf_until(/,/^}/p; /^fn verify_unborn_head_with_leaf_until(/,/^}/p' "$root/crates/phoenix-ide/src/runtime/close_retirement.rs" > src/head-baseline.rs
sed -i '/^fn parse_target_head_reflog_roots(/,$d' src/baseline.rs
rustc --edition 2024 src/head-cli.rs -o head-cli
