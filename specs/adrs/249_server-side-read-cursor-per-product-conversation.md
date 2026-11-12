# ADR-249: Server-side read cursor per ProductConversation

- **Status:** Accepted
- **Date:** 2026-11-12
- **Affects:** REQ-CONV-027

## Context

The user wants conversations to show when they hold output the user has not
seen, and wants to explicitly mark a conversation unread as a "revisit later"
workflow. This must cover ordinary ProductConversations and the Global
Coordinator. Phoenix is a server that several clients (browser tabs, the iOS
client, `phoenix-client.py`) view concurrently, and a ProductConversation spans
several transcript rows once it continues.

## Options considered

1. **Browser-local read state (localStorage).** No schema change, but state
   diverges per device and tab, and a fresh browser marks everything unread.
2. **Server-side read cursor keyed by transcript row.** Shared across clients,
   but continuation moves the head to a new row, so the cursor must be migrated
   or every continuation reads as a new conversation.
3. **Server-side read cursor keyed by ProductConversation.** One row per
   aggregate (ordinary or Coordinator) holding the message the user last read
   through plus an explicit unread mark.

## Decision

Option 3. `product_conversation_read_state` holds
`read_through_message_id` and `marked_unread`. Viewing records the head
transcript row's latest message as the cursor and clears the mark. A
conversation is unread when it is marked, or when its latest head message is
not user-authored and differs from the cursor. Comparing identity rather than
order avoids depending on cross-row message ordering or timestamps.

The cursor is recorded at the server's latest message rather than at an id the
client rendered, because client transcripts omit represented continuation
openings and so cannot always name the true latest message. Clients only
request read-marking while the conversation is visible.

## Consequences

- **Positive:** read state is shared by every client and survives
  continuation; the Coordinator needs no special case.
- **Negative:** a message landing between render and the read request is
  treated as read; a user message from another conversation landing last hides
  earlier unseen agent output until the agent replies.
- **Neutral:** the migration seeds every existing ProductConversation as read
  so upgrade does not flood the sidebar.

## References

- `Database::list_product_conversation_read_states`,
  `Database::mark_product_conversation_read`,
  `Database::mark_product_conversation_unread`
- `useAutoMarkProductConversationRead` (UI)
- `specs/conversation-ui/requirements.md` REQ-CONV-027
