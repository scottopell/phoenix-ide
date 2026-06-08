# Stabilize MessageList Virtuoso slots

Task 51001 identified candidate C15 as plausible but requiring focused Virtuoso validation: `SystemPromptHeaderSlot` and `EmptyPlaceholder` component types are created inside render memo blocks, so system-prompt expansion may create new slot component identities and remount/churn Virtuoso slots.

Evidence to gather:
- Toggle system prompt expansion in a populated conversation.
- Inspect Virtuoso header remount/render behavior and layout work.
- Measure whether churn extends beyond the header itself.

Acceptance criteria:
- If validated, use stable slot component types with data passed through an API Virtuoso supports.
- Preserve system prompt expansion behavior and empty-list placeholder behavior.
