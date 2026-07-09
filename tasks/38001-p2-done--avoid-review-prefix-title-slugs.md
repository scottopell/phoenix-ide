# Avoid Overused Review Prefix in Generated Conversation Slugs

## Request

Update conversation slug generation so generated slugs do not overuse `review-*` as a prefix.

## Plan

1. Update `TITLE_PROMPT` in `crates/phoenix-core/src/llm_language.rs` to explicitly discourage generic `Review ...` titles and require more specific action/object wording.
2. Add examples that steer the model away from `Review <thing>` and toward concrete summaries such as `Update Title Generation Prompt`.
3. Run the relevant tests/checks for prompt/title generation if available.

## Clarification

The code calls this conversation “title generation,” but `generate_title` immediately passes the model output through `sanitize_title`, producing the kebab-case conversation slug. So updating `TITLE_PROMPT` is the prompt-side control for avoiding `review-*` slugs.

## Non-goals

No sanitizer fallback or post-processing rule for `review-*`; this change is prompt-only.
