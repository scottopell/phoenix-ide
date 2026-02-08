# Prose Reader - Trigger Requirements Analysis

## Current Spec Requirements (from requirements.md)

### How Prose Reader is SUPPOSED to be triggered:

#### Primary Flow (REQ-PF-001):
```
WHEN user taps the file browse button in the conversation
THE SYSTEM SHALL display a file browser overlay
...
WHEN user taps a text file (any extension)
THE SYSTEM SHALL open the prose reader for that file
```

**Key phrase:** "file browse button in the conversation"

**Implied trigger location:**
- Button in message input toolbar
- OR button in conversation actions menu
- OR keyboard shortcut (e.g., Ctrl+O)
- The spec says "file browse button" but doesn't specify WHERE exactly

#### Secondary Flow (REQ-PF-014):
```
WHEN patch tool generates output with unified diffs
THE SYSTEM SHALL extract all unique filenames mentioned in the diffs
AND display them as a clickable list at the end of the patch output
AND show count of changes per file (e.g., "file.rs (3 changes)")

WHEN user clicks/taps a filename from the extracted list
THE SYSTEM SHALL open that file in the prose reader
```

**Trigger:** Clickable filenames embedded in patch output

---

## Integration Points (from design.md)

### Conversation UI Integration Section:

```typescript
// The parent component manages state:
const [showFileBrowser, setShowFileBrowser] = useState(false);
const [proseReaderPath, setProseReaderPath] = useState<string | null>(null);

const handleFileSelect = (filePath: string) => {
  setShowFileBrowser(false);
  setProseReaderPath(filePath);
};
```

**Quoted from design.md:**
> "The conversation page needs a button to open the file browser. This could be:
> - A button in the message input toolbar
> - A menu item in a conversation actions menu
> - A keyboard shortcut (e.g., Ctrl+O)"

**NOTE:** This is OPTIONAL/SUGGESTED. The spec leaves it vague.

---

## CURRENT IMPLEMENTATION PROBLEM

**From the bug report you created:**
> "On desktop, there is no clear/visible way to discover how to trigger the 'add note' feature."

This is actually **two related problems:**

1. **Primary issue:** No visible way to OPEN the prose reader at all
   - Spec says "file browse button" but doesn't specify location
   - Design doc suggests 3 possible implementations
   - Current implementation may not have implemented ANY of them

2. **Secondary issue:** Once prose reader is open, "Add Note" trigger is unclear
   - Long-press gesture works on mobile (500ms hold)
   - Desktop support is undefined in spec
   - Should have: mouse interaction equivalent, help text, or keyboard shortcut

---

## What the Spec SHOULD Say (Missing Requirements)

### Desktop-Specific Trigger Requirements

**REQ-PF-D01: Desktop Prose Reader Discovery**
```
WHEN user is in a conversation on desktop
THE SYSTEM SHALL provide a discoverable mechanism to open the file browser:
  - OPTION A: A prominent button in the message input toolbar labeled "Browse Files" or "📁"
  - OPTION B: A keyboard shortcut (e.g., Ctrl+O or Ctrl+Shift+O) with hint text displayed
  - OPTION C: A context menu option accessible via right-click or menu button

WHEN user hovers over the file browser button on desktop
THE SYSTEM SHALL display tooltip: "Browse and review project files"

WHEN user has not yet discovered the feature
THE SYSTEM SHALL show a help icon (?) or first-run hint
```

**REQ-PF-D02: Desktop Annotation Trigger**
```
WHEN prose reader is open on desktop
AND user hovers over a line of text
THE SYSTEM SHALL show a visual affordance (e.g., highlight, icon, or gutter indicator)

WHEN user hovers over a line
THE SYSTEM SHALL display either:
  - A "Comment" icon that user can click
  - OR help text showing: "Click to annotate" or "Press Ctrl+Alt+N"
  - OR a keyboard shortcut hint: "[Click or Ctrl+Alt+N to annotate]"

WHEN user right-clicks on a line
THE SYSTEM SHALL show context menu with "Add Note" option
```

---

## Summary

The prose reader spec has **two levels of missing requirements:**

1. **Vague opening trigger** - "file browse button in the conversation" is not specific
   - Design doc suggests 3 approaches but doesn't mandate one
   - Current implementation gap: no visible button on desktop

2. **No desktop annotation trigger** - Spec assumes mobile long-press works everywhere
   - Desktop needs: hover state, icon affordance, or context menu
   - Mobile long-press is invisible to desktop users
   - No keyboard shortcut documented for "Add Note"

**Both bugs are spec compliance issues, not just implementation bugs.**
