`migrations-through-129.rs.gz` is the byte-exact
`crates/phoenix-db/src/migrations.rs` git object from commit
`9ebd9d8d42876c3356557f4ab461b3b8ffabd743`, compressed with `gzip -n`.
Its decompressed SHA-256 is
`c54f9884b1b86aea19ca214cf059724f456c899a219006b59f1c6a16cca6caa5`.
Deployment tests decompress it without relying on historical git objects in
shallow CI clones. It provides immutable historical protected-route evidence;
synthetic schema-128 fixtures remove migration129 and scope their own pin.
