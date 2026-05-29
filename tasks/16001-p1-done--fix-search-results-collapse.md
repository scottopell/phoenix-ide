# Fix search results vertical collapse

## Problem

The new rendered search output can collapse vertically when many result groups are shown inside a height-constrained conversation/tool block. In the reported screenshot, file result cards compress into thin strips instead of preserving readable row heights and scrolling within the result list.

## Likely cause

`ui/src/index.css` renders `.search-results-list` and `.keyword-search-list` as column flex containers with `max-height` and `overflow-y: auto`, but their result children (`.search-results-file`, `.keyword-search-hit`, and possibly per-line buttons) do not set `flex-shrink: 0` or an equivalent minimum-size policy. Under ancestor height constraints, flex items can shrink vertically, producing the collapsed strips.

## Plan

1. Update the search result CSS so scrollable result-list children keep their intrinsic/readable height:
   - set `flex: 0 0 auto` or `flex-shrink: 0` on `.search-results-file` and `.keyword-search-hit`;
   - consider `min-height: max-content`/targeted line-height only if needed after testing;
   - ensure `.search-result-line` remains readable and does not collapse.
2. Add a regression test for the expected non-shrinking CSS contract, likely in `ToolOutputRenderers.test.tsx` or a lightweight CSS contract test if existing patterns support it.
3. Manually verify with a large search result payload that the list scrolls and result cards no longer flatten.
4. Run targeted UI tests, then `./dev.py check` if feasible.

## Acceptance criteria

- Search result groups retain readable vertical height when there are many matches.
- Overflow is handled by scrolling the result list, not by compressing result cards/rows.
- Both `search` grouped results and `keyword_search` rendered results are covered.
