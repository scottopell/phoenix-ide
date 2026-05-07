---
created: 2026-05-07
priority: p2
status: ready
artifact: ui/src/pages/ConversationPage.tsx
---

Auto-continuation: when a conversation hits context_exhausted state, automatically call continueConversation(), send the summary as the first message to the new conversation, and navigate to it — no user interaction required.

Add a useEffect in ui/src/pages/ConversationPage.tsx that fires when convStateForChildren.type transitions to "context_exhausted". Guards: skip if conversation.continued_in_conv_id is already set (user navigated back to a parent, do not auto-navigate away), skip if the ref already recorded this conversation ID (prevent double-fire on re-renders). Sequence: api.continueConversation(id) → api.sendMessage(newConvId, summary, [], crypto.randomUUID()) → navigate(/c/${slug}). On failure, reset the ref so the existing manual banner remains as fallback. Declare one autoContinuedRef = useRef<string | null>(null) alongside the other refs in the component. No Rust changes required — the sendMessage queuing infrastructure already handles the case where the message arrives before the runtime is fully warmed up.
