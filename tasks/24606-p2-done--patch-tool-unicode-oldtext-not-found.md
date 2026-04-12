---
created: 2026-03-29
priority: p2
status: done
artifact: src/tools/patch.rs
---

# patch tool: `oldText not found` when text contains multi-byte UTF-8 characters

## Summary

The `patch` tool reports `OldTextNotFound` for `oldText` strings that contain
multi-byte UTF-8 characters (em dashes `—`, curly quotes `‘’“”`, ellipsis `…`, etc.) even
when the text visually appears in the file. All three matching strategies in
`find_unique_match` fail silently on these inputs.

## Observed behaviour

Triggered on git `fe826c6` while patching
`specs/nexrad-radar/design.md`. The file contains em dashes written as the
three-byte sequence `\xe2\x80\x94`. The `oldText` argument supplied to the tool
also contained em dashes, but `content.match_indices(old_text)` returned zero
matches and the tool responded:

```
Error: oldText not found in file
```

Switching to a Python `sed -n` inspection confirmed the bytes are identical
in both the file and the intended search string — the match *should* succeed.
A workaround of splitting the patch into smaller hunks that avoid the
problem characters unblocked the operation, but required multiple retries.

## Root cause (hypothesis)

`src/tools/patch/matching.rs` — `find_exact_unique` (line 44):

```rust
let matches: Vec<_> = content.match_indices(old_text).collect();
```

`str::match_indices` is a byte-level scan and handles UTF-8 correctly *if* the
byte sequences are identical. The failure therefore likely originates upstream:
the LLM-generated `oldText` JSON string is deserialised through `serde_json`,
which unescapes `\uXXXX` sequences. If the source file was written with
literal UTF-8 bytes but the JSON encodes the same codepoint as a `\uXXXX`
escape (or vice versa), both paths produce the same Rust `&str` and should
match. However, if the file contains a lookalike codepoint (e.g. a regular
hyphen-minus `\x2d` vs. en dash `\xe2\x80\x93` vs. em dash `\xe2\x80\x94`),
or if the `newText` of a previous patch introduced a different codepoint than
what the LLM later tries to match, the byte sequences genuinely differ and
no match is found.

The fallback strategies (`find_dedent_match`, `find_trimmed_match`) do not
attempt Unicode normalisation or lookalike substitution, so they also fail.

## Acceptance criteria

- [ ] Add a regression test in `src/tools/patch/matching.rs` that asserts
      `find_unique_match` succeeds when `old_text` contains em dashes, curly
      quotes, and ellipsis characters that are byte-identical to those in
      `content`.
- [ ] Add a test that asserts a *clear, actionable error message* is returned
      when the failure is likely a Unicode lookalike (detected by finding a
      match after Unicode NFKC normalisation of both strings).
- [ ] If NFKC-normalised strings match but raw bytes do not, return
      `PatchError::UnicodeMismatch { found_after_normalisation: true }` (or
      equivalent) with a message such as: *“oldText not found — a visually
      similar match exists after Unicode normalisation; check for lookalike
      characters (e.g. em dash vs hyphen)”*.
- [ ] If no match exists even after NFKC normalisation, retain the existing
      `OldTextNotFound` error.
- [ ] All existing tests in `matching.rs` continue to pass.

## Notes

The `unicode-normalization` crate is the standard choice for NFKC in Rust;
check `Cargo.toml` before adding it — it may already be a transitive
dependency. The normalisation check only needs to run as a diagnostic fallback
after the three existing strategies fail, so it adds no cost to the happy path.
