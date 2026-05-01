---
created: 2026-04-23
priority: p2
status: done
artifact: src/message_expander.rs
---

# Problem

`looks_like_file_path` and the `@` tokenizer are markdown-blind. Inline
backticks are safe by accident (the char before `@` is a backtick, not
whitespace, so the sigil is skipped), but fenced code blocks, indented
code, and blockquotes all let `@` sigils leak through and get expanded as
file references.

Concrete failure shapes the user hits today:

```text
Looks good, here is the trace:
\`\`\`
panic at @src/main.rs:42
\`\`\`
```

The `@src/main.rs:42` inside the fence gets tokenized (newline before it is
whitespace) and then classified as a file path (contains `/`). The expander
either reads the file and injects its contents into the LLM text (wrong —
the fence was supposed to be literal), or errors with FileNotFound if the
file is stale. Either way the LLM receives text the user did not intend.

This is the pragmatic halfway point between the current heuristic and the
correct-by-construction answer (structured references on the message model,
a much bigger refactor). It closes the code-fence bleed without changing
syntax, UX, or storage.

## Proposal

Mask markdown code regions before tokenizing, so sigils inside code never
surface as references.

Use `pulldown-cmark` (already widely used in the Rust ecosystem, small,
no JS dep). Walk the events, collect byte ranges for:

- `Event::Code(_)` — inline code spans
- `Tag::CodeBlock(_)` .. `TagEnd::CodeBlock` — fenced AND indented
- `Tag::Html(_)` / `Event::Html` — inline + block HTML
  (optional, lower priority — HTML in user messages is rare)

Produce a sorted `Vec<Range<usize>>` of masked byte ranges. In
`tokenize_references`, after the whitespace-boundary check for a sigil at
offset `i`, also skip if `i` falls inside any masked range. Binary search
the sorted ranges (O(log n) per sigil) — worst-case message is tiny so
either that or a linear scan is fine.

Crucially: the masking only affects tokenization. Display text and llm
text still carry the original bytes verbatim — we are not rewriting the
message, just refusing to treat sigils inside code as references.

## Why not full structured references (Option D)

D is the right long-term shape but requires rewriting the message model,
DB schema, autocomplete flow, and textbox rendering. A is a narrow
additive change that closes the observed bug class without committing to
that larger refactor. Filed as a parallel task when we are ready.

## Correct-by-construction notes

This does *not* eliminate `looks_like_file_path` — the heuristic still
decides `@username` vs `@src/main.rs` outside code. Option A improves the
*scope* of where the heuristic runs (only in prose, never in code), which
is where the heuristic belongs.

Add one new invariant to the inline-references Allium spec:

- `NoExpansionInsideCode`: for all sigil positions `p` in `text`, if
  `p` is contained in a markdown code region of `text`, then no
  `InlineReference` with `span.start == p` appears in the tokenizer
  output.

## Acceptance

- `pulldown-cmark` added as a dep in `Cargo.toml` (use the default
  feature set; we only need the event stream).
- New helper `fn masked_code_ranges(text: &str) -> Vec<Range<usize>>` in
  `src/message_expander.rs` returning sorted, non-overlapping byte ranges
  for inline code + code blocks. Unit-tested in isolation.
- `tokenize_references` takes the masked ranges as an additional param
  (or calls the helper itself) and skips sigils that fall inside them.
- New tests:
  - Sigil inside triple-backtick fence: not expanded.
  - Sigil inside inline `` `@foo.rs` `` code: not expanded (already
    works by accident via the whitespace-boundary rule — confirm and
    document as regression coverage).
  - Sigil inside indented (4-space) code block: not expanded.
  - Sigil in prose immediately after a code block closes: still expanded.
  - Sigil in prose before a code block opens: still expanded.
  - Mixed message with some sigils in code and some outside: only the
    outside ones expand.
  - Unclosed fence (no terminating ```): pulldown-cmark auto-closes at
    EOF, verify behavior is conservative (mask to EOF) and document it.
  - Empty string, no-sigil message, no-code message: all degenerate to
    current behavior.
- Allium invariant `NoExpansionInsideCode` added to
  `specs/inline-references/inline-references.allium`.
- `./dev.py check` green.
- Manual smoke in the dev UI: paste a fenced block containing
  `@some_ref`, send, confirm no expansion error and the raw text reaches
  the LLM unchanged.

## Non-goals

- Not adding structured `@{...}` syntax (reserved for Option C if ever
  pursued).
- Not changing the message model or DB schema (Option D territory).
- Not attempting full CommonMark conformance — edge cases like `~~~`
  alternative fences, nested fences, and exotic HTML blocks are fine to
  leave as best-effort from pulldown-cmark's output.
- Not masking block quotes, tables, or other markdown structure —
  sigils in those contexts are usually intentional.
- Not touching `/` skill-invocation tokenization (same codepath, same
  masking benefit — but the existing skill resolver already fails gently
  if a name has no match, so the bleed is cosmetic, not load-bearing).

## Risk / rollback

Low risk. The change is additive: if `masked_code_ranges` returned an
empty vec, behavior would be identical to today. Rollback = revert the
commit; no schema or wire changes.
