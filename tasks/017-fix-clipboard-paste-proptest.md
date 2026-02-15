---
id: 017
title: Fix flaky prop_clipboard_cut_paste_preserves test
status: ready
priority: p3
created: 2025-02-15
---

# Fix Flaky Clipboard Proptest

## Problem

The `prop_clipboard_cut_paste_preserves` property test in `src/tools/patch/proptests.rs` is failing with:

```
minimal failing input: path = "a.txt", prefix = "0i_", middle = "i_i", suffix = "a00"
assertion failed: `(left == right)` 
  left: `Some("0_ia00")`,
 right: `Some("0i_a00")`
```

## Analysis

The test creates content `{prefix}{middle}{suffix}` = `0i_i_ia00` and expects to cut `middle` (`i_i`) 
and paste it back, getting the same result. But the content becomes `0_ia00` instead of `0i_a00`.

This suggests that when `middle` contains characters that also appear in `prefix` or `suffix`,
the fuzzy matching in the patch tool is finding a different occurrence than intended.

## Location

- Test: `src/tools/patch/proptests.rs:160`
- Regression file: `proptest-regressions/tools/patch/proptests.txt`

## Fix Options

1. Strengthen the test's constraints to avoid ambiguous matching scenarios
2. Fix the patch tool's matching logic to be more precise
3. Adjust the test to account for this edge case
