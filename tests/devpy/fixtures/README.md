`migrations-through-129.rs.gz` is the byte-exact
`crates/phoenix-db/src/migrations.rs` git object from commit
`9ebd9d8d42876c3356557f4ab461b3b8ffabd743`, compressed with `gzip -n`.
Its decompressed SHA-256 is
`c54f9884b1b86aea19ca214cf059724f456c899a219006b59f1c6a16cca6caa5`.
Deployment tests decompress it without relying on historical git objects in
shallow CI clones. It provides immutable historical protected-route evidence;
synthetic schema-128 fixtures remove migration129 and scope their own pin.

`migrations-through-131.rs.gz` is the byte-exact target-131 Rust registry from
commit `3c83092665f951cb01547eb79a1e952e4776c88f`, compressed with gzip (`mtime=0`).
Its uncompressed SHA-256 is
`850c25de411d57f14d07cd3fb09e3ee15e9e89f4fd07b24d48de00a533f1e1ac`.
`OrdinarySource129To131Tests` uses it for historical admission and recovery.
`GenericOrdinaryForwardHistoryTests` exercises newer ambient registries under
the generic exact-prefix admission contract, separately from historical evidence.

`migrations-through-140-2d66.rs.gz` is the byte-exact
`crates/phoenix-db/src/migrations.rs` git object from commit `2d66`, compressed
with `gzip -n`. Its decompressed SHA-256 is
`f2dd8cf9c30438ead4848d3fec21bd90813badc8928bf6f99dbae82d0e620dc4`.
`GenericOrdinaryForwardHistoryTests` verifies its immutable digest and exact
129/131-to-140 admission separately from the current migration registry.

`migrations-through-140.rs.gz` is separate target-140 registry evidence used by
the generic ordinary-forward history tests. Its decompressed SHA-256 is
`e0641e86e0c05aef9003f190990046d37549b52083727c4c0651572b7a9ce33e`.
Both target-140 artifacts remain immutable; neither substitutes for the current
candidate registry.
