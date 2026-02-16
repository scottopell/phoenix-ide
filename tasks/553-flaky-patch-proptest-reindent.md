---
status: done
priority: p2
created: 2025-02-16
---

# Flaky Proptest: prop_reindent_roundtrip Too Many Global Rejects

## Summary

The `prop_reindent_roundtrip` proptest in `src/tools/patch/proptests.rs` fails when run with high iteration counts (PROPTEST_CASES > ~4000) due to excessive test case rejection.

**Failure boundary:** Passes at PROPTEST_CASES ≤ 4000, consistently fails at PROPTEST_CASES ≥ 4500.

## Root Cause

The test uses `prop_assume!(!indent.is_empty())` to filter generated test cases, but the strategy that generates indent values produces empty strings 20% of the time:

```rust
fn arb_indent() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),       // 20% chance
        Just("  ".to_string()),     // 20% chance
        Just("    ".to_string()),   // 20% chance
        Just("\t".to_string()),     // 20% chance
        Just("\t\t".to_string()),   // 20% chance
    ]
}
```

With the test configured for `ProptestConfig::with_cases(500)`, at default case counts the rejection happens but doesn't cause failure. However, when PROPTEST_CASES is increased to 5000+, the accumulated rejections exceed proptest's tolerance (~1024 consecutive global rejects).

## Failure Pattern

Running with PROPTEST_CASES=5000:
```
Test aborted: Too many global rejects
    successes: 3970-4213
    global rejects: 1024
        1024 times at src/tools/patch/proptests.rs:240:9: !indent.is_empty()
```

The test succeeds ~4000 cases but fails consistently due to rejection rate.

## Test Location

- File: `src/tools/patch/proptests.rs`
- Lines: 231-254 (test definition)
- Strategy: `arb_indent()` at lines 38-46
- Assumption: Line 240 `prop_assume!(!indent.is_empty())`

## Reproduction

### Quick test (passes):
```bash
cd /home/exedev/phoenix-ide
cargo test --bin phoenix_ide patch::proptests::prop_reindent_roundtrip --release
# Default config at 500 cases: PASS ✓
```

### Trigger failure (consistent):
```bash
PROPTEST_CASES=5000 cargo test --bin phoenix_ide patch::proptests::prop_reindent_roundtrip --release
# Always fails at high case counts: FAIL ✗
```

### Boundary testing:
- PROPTEST_CASES=1000: PASS ✓
- PROPTEST_CASES=2000: PASS ✓  
- PROPTEST_CASES=3000: PASS ✓
- PROPTEST_CASES=4000: PASS ✓
- PROPTEST_CASES=4500: FAIL ✗ (first failure point)
- PROPTEST_CASES=5000: FAIL ✗

## Why This is a Problem

1. **Proptest Philosophy**: The assumption/rejection pattern is a code smell in property testing. Using `prop_assume!` to filter out 20% of generated cases is inefficient and fragile.

2. **Scale Sensitivity**: The test passes at 500 cases but fails at 5000+ cases, making it vulnerable to changes in test configuration or CI settings that might increase iteration counts.

3. **Regression File**: When the test fails, proptest creates regression files (`proptest-regressions/tools/patch/`) to reproduce failures. However, these are often deleted during cleanup, making the issue hard to track.

## Notes for Investigation

The problem is NOT in the patch logic itself—the assumption is rejecting cases that are valid test scenarios (indent strings that ARE non-empty). The strategy just has a poor distribution.

DO NOT FIX YET—this task is for understanding the issue, not solving it.
