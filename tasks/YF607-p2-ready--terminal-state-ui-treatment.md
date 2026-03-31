---
created: 2026-03-13
priority: p2
status: ready
artifact: pending
---

# Clear visual treatment for Terminal conversations

## Problem

After Complete or Abandon, the conversation goes to Terminal state but the UI
doesn't clearly communicate this. The input area still renders (though messages
are rejected server-side). The sidebar shows "Explore" (because conv_mode is
reset). There's no banner or visual cue that the conversation is finished.

## What to Do

1. **Disable/replace input area**: When `atom.phase.type === 'terminal'`, replace
   the InputArea with a static banner: "This conversation has ended." or show the
   system message (commit SHA for complete, "abandoned" for abandon) prominently.

2. **Sidebar badge**: Terminal conversations should show a distinct badge --
   maybe "Done" or a checkmark icon instead of "Explore". Derive from
   `getDisplayState()` returning `'terminal'`.

3. **Hide WorkActions**: Already handled (phaseType check), but verify there's
   no flash of the actions bar before the terminal state propagates.

4. **Muted visual style**: Consider dimming/muting the message list slightly
   for terminal conversations to signal read-only. Subtle, not aggressive.

## Acceptance Criteria

- [ ] Terminal conversations show a completion banner instead of input area
- [ ] Sidebar shows "Done" or equivalent badge for terminal conversations
- [ ] No way to accidentally send messages to a terminal conversation
- [ ] Visual distinction between active and terminal conversations is clear at a glance
