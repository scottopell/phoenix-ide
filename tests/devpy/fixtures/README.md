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
`OrdinarySource129To131Tests` uses it for historical admission and recovery;
the ambient target-140 registry is exercised separately as an unqualified
interval that must be rejected before any service stop or start.
