# Avoid false-positive @file expansion for decorators and code-like tokens

## Problem

A `/new` message draft containing a FastAPI decorator such as:

```python
@app.get("/.well-known/api-catalog", include_in_schema=False)
```

is rejected before conversation creation with an expansion error like:

```text
File not found: app.get("/.well-known/api-catalog",
```

The backend inline-reference expander tokenizes `@app.get("/.well-known/api-catalog",` as an `@` token. Because the token contains `/`, `looks_like_file_path()` classifies it as an intentional file expansion reference, then tries to resolve it as a file.

This violates the user-facing intent of REQ-IR-007: non-file `@` usages in code snippets, annotations, decorators, and mention-like text should pass through literally instead of blocking send.

## Proposed fix

1. Tighten backend `@` file-reference classification in `crates/phoenix-ide/src/message_expander.rs`:
   - Keep genuine file references working:
     - `@src/main.rs`
     - `@AGENTS.md`
     - `@foo/bar`
     - relative path shapes that Phoenix already supports.
   - Treat code/decorator/function-call shaped tokens as literals.
   - Define `@file` classification around an accepted path-token shape, with a small deny set for characters that cannot be part of Phoenix's intended inline path references and strongly indicate code/prose syntax (for example call punctuation, quotes, delimiters, or trailing prose punctuation).
   - Ensure any dealbreaker character in the token prevents file classification even if the token also contains `/` or a known extension.
   - Prefer a small structural predicate over a large blacklist: accepted `@file` tokens should look like path tokens, not arbitrary prose/code that happens to contain `/`.

2. Add regression tests:
   - `@app.get("/.well-known/api-catalog",` passes through unchanged.
   - A full pasted FastAPI route snippet in a normal prose prompt does not error.
   - Existing positive cases still error/expand as before (`@src/main.rs`, `@AGENTS.md`, existing text file expansion).
   - Existing false-positive exclusions still pass (`@username`, `@param`, Bazel labels, URLs).

3. Update `specs/inline-references/requirements.md` and `specs/inline-references/inline-references.allium` if needed so the normative heuristic no longer says every token containing `/` is automatically a file reference. The timeless rule should be: `@` expands only when the token has a valid path-token shape, while code/decorator tokens are literal.

4. Run focused tests, then broader project checks as appropriate:
   - `cargo test message_expander`
   - relevant UI/API tests only if the API response shape changes (it likely should not).

## Non-goals

- Do not remove `@file` expansion.
- Do not require users to escape ordinary pasted code snippets.
- Do not change `./path` literal path behavior.
- Do not change autocomplete insertion semantics unless implementation uncovers a frontend-only inconsistency.
