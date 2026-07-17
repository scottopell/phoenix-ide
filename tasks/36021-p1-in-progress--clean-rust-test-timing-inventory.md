# Clean the existing Rust test timing inventory

Classify every finding from `scripts/check_rust_test_timing.py --all crates/` as a removable timing bet, an intentional behavior driver, an already-bounded false positive, or an architectural synchronization gap. Land deterministic fixes in small subsystem tranches, use local `test-timing-allow` reasons only when elapsed time itself is the tested behavior, and file explicit follow-ups for concurrency seams that require production readiness signals. Completion requires an empty `--all` inventory without broad allowlists.
