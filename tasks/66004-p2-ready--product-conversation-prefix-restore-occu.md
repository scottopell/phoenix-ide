# Restore ProductConversation reader anchors by occurrence identity

ProductConversation aggregate messages carry `productOccurrenceToken` so repeated message IDs in different transcript-row occurrences remain distinct, but prefix-expansion restoration still records only `messageId`. If an older page prepends another occurrence with the same message ID as the reader's current anchor, restoration can resolve the wrong segment occurrence.

Reproduce with two ProductConversation segments containing the same message ID, capture the newer occurrence as the reader anchor, prepend the older occurrence, and prove the viewport remains on the newer occurrence. Thread the existing occurrence identity through the restore basis/command and retain ordinary-conversation behavior. Read the conversation scroll/history and chains/bedrock specs first; use the real ProductConversation fixture journey and do not change persistence.
