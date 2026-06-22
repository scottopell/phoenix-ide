# Improve patch tool duplicate-match diagnostics

## Context

The patch tool is safe and workable, but a common agent workflow hits a sharp edge: duplicate `oldText` failures only report a count, not where the matches are. That preserves safety, but forces an extra read/search cycle before the agent can widen the replacement text.

The highest-value improvement is better duplicate-match diagnostics: when `oldText` appears more than once, the tool should return matching line numbers and short snippets so the agent can widen the replacement text without an extra search/read cycle.

## Scope

When a replace patch fails because `oldText` is non-unique, return an error that includes:

- the total match count;
- up to a bounded number of match locations, preferably all for small counts and a capped list for large counts;
- for each reported match:
  - 1-based start line number;
  - a short surrounding snippet or the matched line(s), with enough context to disambiguate repeated match arms/test blocks;
- the existing guidance about widening `oldText` or splitting patch calls.

Implementation sketch:

- Extend the pure matching layer to produce duplicate-match diagnostics without losing the existing `PatchError` safety semantics.
- Keep diagnostics bounded so errors do not explode on very common strings.
- Preserve exact-match behavior: if a later fuzzy strategy finds a unique safe match, the patch should still apply as it does today.
- Add unit tests in the matching/planner layer and a tool-level test proving the user-facing error includes line numbers/snippets.

## Out of scope

Do not add:

- dry-run / preview mode;
- non-atomic partial apply mode;
- post-apply context output;
- sequential multi-patch mode.

Sequential multi-patch mode may be worth considering separately in the future, but this task keeps the current atomic simultaneous default and only improves failure diagnostics.

## Requirements / spec updates

Update `specs/patch/requirements.md` and `specs/patch/executive.md` to describe the new duplicate-match diagnostics.

Do not weaken the existing uniqueness guarantee for replace operations.

## Validation

- Add/adjust Rust tests for duplicate diagnostics, including repeated single-line and multi-line snippets.
- Verify error output is concise but actionable.
- Run the relevant Rust tests for `phoenix-tools` / patch modules.
- Run `./dev.py check` if the change touches generated types, specs, or broader tool behavior.
