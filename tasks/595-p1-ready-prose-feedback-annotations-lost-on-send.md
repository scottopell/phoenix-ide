---
number: 595
priority: p1
status: ready
slug: prose-feedback-annotations-lost-on-send
---

# Prose Feedback: Annotations Lost on Send

## Description

User reported that annotations written in the prose feedback UI are silently lost
when the send icon is tapped. The formatted message is not injected into the
message input and the notes are cleared with no visible output.

## Reproduction Steps

1. Open a file in the prose reader (long-press any line or use file browser)
2. Long-press a line to open the annotation dialog
3. Type a note and tap Add Note
4. Verify the note badge appears in the header (note count > 0)
5. Tap the send icon
6. Observe: notes disappear, nothing appears in the message input

## Expected Behavior

Per REQ-PF-009: formatted notes should be injected into the message input field
as a structured message with file path, line numbers, and raw line content.
Notes should be cleared AFTER successful injection. Message input should be
focused.

## Likely Suspects

- Send handler not finding or targeting the correct message input element
- State update ordering: notes cleared before injection completes
- Event not reaching the parent component (overlay/portal boundary)
- The send icon button's onClick wired to wrong handler or missing handler
- Message input selector mismatch between ProseReader and the actual input DOM node

## Investigation Starting Points

```
ui/src/components/ProseReader.tsx  (or similar path)
ui/src/components/          (grep for onSend, handleSend, sendNotes)
```

## Priority Rationale

p1 — this is a complete loss of user work with no recovery. The feature appears
to work (note badge shows) but silently discards the output. High chance of user
not noticing until they see the agent hasn't received their feedback.
