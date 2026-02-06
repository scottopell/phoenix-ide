# P0: Critical - Messages Sent Twice When Retrying Failed Send

## Severity
**CRITICAL (P0)** - Data corruption/duplication issue affecting user experience

## Issue Description
When a user encounters a "failed to send message" error and clicks retry, the message is sent **twice**, which should be impossible in this application. This indicates a critical bug in the message retry/deduplication logic.

### Steps to Reproduce
1. Send a message that fails to send
2. See "failed to send message" error notification
3. Click the "Retry" button
4. Observe: Message appears twice in the conversation

### Expected Behavior
- Clicking retry should send the message exactly once
- Either the message was already sent (no duplicate), or retry sends it once
- No scenario should result in duplicate messages

### Actual Behavior
- Message is sent twice in the conversation
- Both copies appear in the UI and database

## Root Cause Analysis Needed
- Check message submission handler for deduplication logic
- Verify retry mechanism doesn't bypass idempotency checks
- Review message ID generation and uniqueness guarantees
- Audit database constraints for duplicate detection

## Related Logs
See: `/home/exedev/phoenix-ide/phoenix.log`
Dev server logs: `/tmp/dev_server.log`

## Investigation Areas
1. **Message submission API** - Check for idempotent handling
2. **Client retry logic** - Ensure same message ID on retry
3. **Database constraints** - Verify unique constraints on (conv_id, message_id)
4. **UI state management** - Check for optimistic updates causing duplicates
5. **Error handling flow** - Trace failed send → retry path

## Acceptance Criteria
- [ ] Root cause identified and documented
- [ ] Fix prevents duplicate messages on retry
- [ ] Existing duplicate messages can be cleaned up
- [ ] Idempotency guarantee documented in code
- [ ] Test cases added for retry scenarios
- [ ] Verified against all message types (text, tool calls, etc.)

## Impact
- **Users affected**: All users using retry functionality
- **Data integrity**: Database may contain duplicate messages
- **Trust**: Users lose confidence in message delivery

## Priority
Must be fixed before next release. Block on this until resolved.
