# Fix stale PR reaction status in conversation badges

## Finding

Commit `6aee4d74cbda9d12682e1c7cf4bf1d0c4beee1f4` (`Surface PR feedback reaction status in sidebar badges (#443)`) introduced a lightweight status query for conversation-list badges.

The sidebar query currently crawls aggregate `reactionGroups { content reactors { totalCount } }` across every PR issue comment, review summary, and unresolved review-thread comment. `reaction_status_from_graphql_value` then maps any positive `EYES` count anywhere in those lifetime surfaces to `PrFeedbackStatus::InProgress`.

This targets the wrong GitHub surface. Review bots conventionally place these workflow reactions on the root pull request description, so comment/review/thread reactions are both unnecessary network work and a source of false positives. An old or unrelated eyes reaction on any traversed comment can persist indefinitely as the present-tense `👀` badge, explaining the suspicious status on PR #470.

Reaction status should therefore have one narrow contract: derive it only from reactions attached to the root pull request/description. Per-comment reaction hydration has no demonstrated consumer value—the Address Feedback workflow already has comment text and thread-resolution state—so it should be removed rather than retained as a second reaction model.

## Plan

1. Reproduce against PR #470 with `gh` and identify the comment/review reaction currently causing the false eyes status. Confirm the workflow reaction state on the root PR description.
2. Replace the three paginated sidebar scans (issue comments, review summaries, and review threads) with one bounded GraphQL query for the root pull request’s reaction groups.
3. Derive reaction status exclusively from root-description reactions. Preserve deterministic precedence between supported root signals (`+1` over `eyes`, unless observed bot behavior demonstrates a more accurate contract).
4. Remove detailed comment/review/thread reaction hydration from the Address Feedback pipeline, including reaction-specific fetches, coverage surfaces, item fields, dedupe/fingerprint handling, artifact text, and tests that no longer serve a consumer. Preserve comment content and thread-resolution behavior.
5. Use the root-derived status wherever a compact PR workflow status is needed, avoiding parallel root-level and item-level reaction representations.
6. Treat a successful root query with no supported reaction as `open`, allowing refresh to clear a previously cached false `in_progress` value. Keep query failure structurally distinct so unavailable data does not overwrite a known status.
7. Add backend regression tests proving that comment, review-summary, and review-thread reactions cannot affect status; root `eyes` and `+1` reactions do; and root-query failure preserves cache semantics. Remove obsolete hydration, pagination, status-walker, coverage, and item-reaction tests. Retain UI tests for rendering the typed backend status.
8. Regenerate API types if the removed item-level fields affect codegen, run targeted Rust/UI tests, and run `./dev.py check`.

## Acceptance criteria

- PR #470’s badge matches the supported reactions on its root PR description.
- Reactions on issue comments, review summaries, and review-thread comments cannot influence the conversation-list badge.
- Sidebar reaction refresh uses one bounded root-PR query rather than paginating all feedback surfaces.
- Supported root reactions have deterministic precedence.
- A successful no-reaction result clears a previously cached false-positive status, while a failed query does not masquerade as `open`.
- Address Feedback continues to receive comment text and thread-resolution state without fetching or storing per-item reactions.
- There is one authoritative reaction-status representation derived from the root PR description.
- Reaction-only coverage surfaces, item metadata, and hydration calls with no remaining consumer are removed.
- Tests demonstrate the reported false positive and the corrected behavior.
