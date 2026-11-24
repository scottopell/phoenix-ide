# Move “Ember is handling” to the top of Open

The collapsed group introduced in PR #1119 sits below every regular conversation, so it’s easy to miss. In `ConversationList`, put the existing “Ember is handling” disclosure and count first in the Open list. When expanded, show its rows directly beneath it, followed by the regular rows.

Keep the count, default collapsed state, remembered expansion, and eligibility rules unchanged. The currently viewed conversation and chats that need the user still stay visible outside the group. Leave History and search alone; this is only a placement change.

Update the existing `ConversationList` tests to check that the disclosure comes first when collapsed and that its rows come before the regular rows when expanded, including after remount. Run the focused UI tests.
