# Align Anthropic private-thinking retention with current upstream guidance

Discovered while answering Scott's 2026-11-23 question about clearing encrypted reasoning after a completed reply. Phoenix currently clears Anthropic private replay on a non-tool final response (anthropic.rs final normalization), at settlement, and on accepted model changes (phoenix-db/src/lib.rs model-setting transaction).

Current Anthropic guidance distinguishes allowed omission outside tool use from the recommendation to preserve complete thinking blocks across turns. The preserved-thinking guide also recommends retaining full history across model switches and switching back, letting the server ignore unreadable blocks for each request. Removing them client-side permanently loses that state. Thus Phoenix's behavior is protocol-valid but does not implement current upstream preserved-thinking recommendations. Earlier conversation claims of correctness applied to required tool-loop continuity, not this broader retention behavior.

Sources checked 2026-11-23:
- https://platform.claude.com/docs/en/build-with-claude/thinking#preserving-thinking-blocks
- https://platform.claude.com/docs/en/build-with-claude/preserved-thinking#switch-models-and-back
- https://github.com/anthropics/anthropic-sdk-python/blob/main/tools.md

Bound the work around authenticated private reasoning retained outside public transcript content. Evaluate retaining completed Anthropic thinking and model-bound history until explicit context replacement, and restoring appropriate state when returning to a model. Account for exact prefix binding, supported server-side drop controls, schema ownership, context/token accounting and cost, retries/restarts, and stale outcomes. Preserve provider privacy boundaries when Gemini or OpenAI takes an intervening turn; do not send Anthropic blocks to unrelated providers.

The existing normative continuation contract explicitly retires active private context at settlement/model switch. Any revised retention policy requires a deliberate normative update and ADR, rather than weakening active-exchange validation or silently keeping stale transport handles. Cover final response -> new user turn, Anthropic -> other provider -> Anthropic, compatible/incompatible Claude model transitions, compaction, and restart with focused persistence/wire/runtime regressions and bounded live qualification.
