# Fix mobile message header layout after copy button addition

## Problem

The recent message copy button addition regressed mobile message layout:

- User message headers are too tall / visually heavy on narrow screens.
- The sent checkmark is pushed into an awkward centered position because it uses `margin-left: auto` in the same header row as the mobile copy button.
- The copy button appears as a large 44px header control, which inflates the message chrome and makes the actual message content feel secondary.

Screenshot evidence shows a mobile user message where `YOU 8:03 AM` is on the left, the green checkmark floats near the horizontal center, and the copy button occupies a large box at the top-right of the message.

## Likely cause

`UserMessageImpl` renders the status and copy button together inside `.message-header`:

```tsx
<span className="message-status sent" ...>✓</span>
<MessageCopyButton ... />
```

CSS then combines:

- `.message-status { margin-left: auto; }`
- mobile `.message-mobile-copy.copy-btn { display: inline-flex; width: 44px; height: 44px; ... }`

This means the status consumes the flexible gap before the copy button rather than sitting naturally with the metadata.

Agent messages already have a separate `.message-mobile-copy-row` path for non-first turn messages, so message-copy placement needs a deliberate mobile layout rather than relying on the generic header row.

## Plan

1. Adjust message header markup/CSS so the status checkmark stays grouped with sender/time metadata, not as the flex spacer before the copy action.
2. Keep the message copy affordance available on touch/mobile, but prevent it from making the header row large.
   - Prefer a compact top-right action style or a separate absolutely-positioned action that does not participate in header flex sizing.
   - Preserve desktop/hover behavior for copy buttons elsewhere.
3. Verify user, queued user, first agent, and non-first agent messages still render correctly.
4. Add or update component/CSS coverage if an existing test fixture covers message rendering layout classes.
5. Run targeted UI checks and `./dev.py check` before committing.

## Acceptance criteria

- On mobile/narrow widths, user message header height returns to the pre-copy compact feel.
- The sent checkmark appears adjacent to message metadata (or otherwise intentionally aligned), not floating in the center of the bubble.
- The copy button remains usable on mobile without dominating the message header.
- Agent message copy behavior remains available and does not regress streaming/finalized message rendering.
- QA screenshots are produced from the relevant Ladle fixture at mobile/narrow width and attached or referenced in the final handoff.
- Existing lint/tests pass.
