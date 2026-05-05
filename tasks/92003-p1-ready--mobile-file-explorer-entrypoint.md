---
created: 2026-05-05
priority: p1
status: ready
artifact: ui/src/pages/ConversationPage.tsx
---
# Define and add mobile file access

## Summary

Mobile conversation view currently has no discoverable way to open project files
unless a file/path link appears in the conversation transcript. The immediate gap
is file access, but the product decision may be broader: mobile may need a general
conversation action surface rather than a one-off Files button.

## Confirmed behavior

- `ConversationPage` renders `FileBrowserOverlay`, but `showFileBrowser` is
  initialized to `false` and there is no call site for `setShowFileBrowser(true)`.
- `DesktopLayout` only renders `FileExplorerPanel` when `isDesktop` is true.
- `DesktopLayout` only renders `CommandPalette` when `isDesktop` is true.
- `CommandPalette` only installs the global Cmd/Ctrl+P shortcut when `isDesktop`
  is true.
- Mobile file opening works after a file is selected: `handleFileSelect` closes
  the browser and opens the existing mobile `ProseReader` overlay.

## User requirements

- A touch-only mobile user must be able to open project files without waiting for
  the assistant to print a file link in the conversation.
- The entry point must be visible or otherwise discoverable in the normal mobile
  conversation view.
- The user must be able to browse from the conversation's current working
  directory, matching the root used by desktop file exploration.
- After choosing a file, the user should land in the existing mobile file reading
  flow unless the selected design intentionally replaces that flow.
- The solution must not regress desktop file explorer or desktop command palette
  behavior.
- Hardware-keyboard shortcuts may improve mobile/tablet support, but they cannot
  be the only mobile access path.

## User journeys

### Journey 1: touch-only file lookup

1. User opens a conversation on a phone.
2. User wants to inspect a project file before replying.
3. User taps a visible mobile control for files or actions.
4. User browses the project tree from the conversation cwd.
5. User selects a file.
6. File opens in the mobile reader.
7. User can close the reader and return to the conversation.

### Journey 2: file lookup while composing

1. User is drafting a message on mobile.
2. User realizes they need to check a file.
3. User opens file access without losing the draft.
4. User reads the file.
5. User returns to the conversation and continues composing.

### Journey 3: broader mobile action access

1. User is in a mobile conversation.
2. User needs a non-chat action such as opening files, searching, or invoking a
   conversation command.
3. User opens a mobile action surface.
4. User chooses Files for this task's initial scope.
5. Future actions can be added without inventing a new entry point each time.

## Candidate UX options

These options are intentionally unsettled. Use them to choose a direction before
implementation.

### Option A: direct Files button

Add a visible Files control in mobile conversation chrome or near the composer.

- Strengths: fastest path to fixing the file access gap; direct and easy to
  understand; can reuse the existing `FileBrowserOverlay`.
- Risks: may create a one-off control for what is really a broader mobile action
  problem; composer-adjacent placement may compete with the primary input.
- Best fit when: the priority is a small, low-risk p1 fix.

### Option B: mobile actions menu

Add a visible mobile action entry point, initially with Files as the first action.

- Strengths: creates a home for mobile-only access to non-chat actions; can later
  include file search, command palette actions, conversation actions, or settings.
- Risks: larger design surface; needs careful scoping so the first version does
  not become a generic menu with unclear ownership.
- Best fit when: this is treated as a mobile navigation/action architecture gap,
  not only a missing file button.

### Option C: mobile command/search entry point

Add a visible mobile command/search affordance that can find files and eventually
mirror desktop command palette behavior.

- Strengths: aligns with desktop Cmd/Ctrl+P concept; supports direct file search
  rather than tree browsing; may solve multiple mobile discoverability gaps.
- Risks: larger implementation and UX burden; search-first may not satisfy users
  who expect to browse the project tree.
- Best fit when: mobile parity with command palette is the desired direction.

### Option D: gesture or keyboard shortcut as secondary access

Support swipe, hardware keyboard shortcut, or similar non-primary access paths.

- Strengths: can make power-user access faster, especially on tablets with
  keyboards.
- Risks: not discoverable enough to satisfy the primary requirement by itself.
- Best fit when: paired with one of the visible touch-first options above.

## Done When

- [ ] A product direction is chosen from the candidate UX options, or a new option
      is documented with equivalent user requirements and journeys.
- [ ] On widths below the desktop breakpoint, a user can open project file access
      without relying on a file link in the conversation transcript.
- [ ] The entry point is touch-accessible and visible/discoverable in a normal
      conversation.
- [ ] Opening file access uses the conversation's current working directory.
- [ ] Selecting a file opens an appropriate mobile file-reading flow.
- [ ] Existing drafts are preserved when opening and closing file access.
- [ ] Desktop behavior is unchanged.
- [ ] Any keyboard shortcut or gesture is additive; touch-only mobile still has a
      visible entry point.
