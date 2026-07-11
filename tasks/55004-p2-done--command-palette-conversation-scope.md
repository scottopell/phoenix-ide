# Add conversation-scoped search to Cmd+P

Support a `c ` prefix in the command palette that scopes completion suggestions to conversations. Typing `c ` shows only conversation suggestions; subsequent text is fuzzy-matched against conversation slugs, so `c emo` naturally selects the best matching `emoji-…` conversation first.

## Behavior

- Keep the existing unprefixed search behavior across all available sources.
- Interpret only a leading `c` followed by whitespace as the conversation scope; ordinary queries beginning with `c` remain ordinary global searches.
- Strip the `c ` scope syntax from the query passed to the conversation source while preserving the user’s raw input in the field.
- With an empty scoped query (`c `), show conversations in the conversation source’s existing default/recency order.
- Query no file or code sources while conversation scope is active.
- Reset selection to the first result whenever the scope or scoped query changes, preserving the existing Enter/click navigation behavior.
- Keep `>` action mode behavior unchanged.

## Implementation

1. Extend the command-palette open state with a typed search scope (global or conversations) and derive it alongside mode/query parsing in `stateMachine.ts`.
2. In `CommandPalette.tsx`, choose eligible sources from that scope before launching async searches. Avoid identifying the source by display category where a stable source ID can be used.
3. Keep conversation ranking in `ConversationSource`: it already fuzzy-matches slugs with exact/prefix/substring/fuzzy ordering and uses recency for the empty query.
4. Update the command-palette input affordance as needed so the scoped syntax remains understandable without hiding what the user typed.

## Verification

- Add pure state-machine tests for `c `, `c emo`, a non-prefix query such as `code`, and unchanged `>` action parsing.
- Add component tests proving `c ` returns only conversations and does not call file/code search APIs.
- Add a component/source test proving `c emo` puts an `emoji-…` slug first and Enter navigates to it.
- Verify clearing/changing the prefix returns to global source search and resets selection.
- Run the relevant UI test suite and project checks.

## Specs

Update `specs/command-palette/requirements.md` with timeless scoped-source behavior and refresh `specs/command-palette/executive.md` to describe the implemented prefix. Do not extend the legacy v1 `design.md`; its normative behavior should move forward through the v2 requirements/current-status artifacts.
