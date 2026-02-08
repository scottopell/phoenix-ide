# P0: Prose Reader Spec - Critical UX Bugs

## Bug #1: Cannot Leave Comments on Table Entries
**Severity:** P0 - Blocks core functionality
**Area:** prose-reader spec, comment system

### Description
Users cannot add comments/annotations to entries within tables. The comment feature appears to work for regular text but fails for table cells or table rows.

### Impact
- Table data is less collaborative - can't annotate table content inline
- Users must work around by commenting on prose around tables instead
- Spec collaboration is hampered for complex data-heavy sections

### Acceptance Criteria
- [ ] Comments can be added to individual table cells
- [ ] Comments can be added to table rows
- [ ] Comments persist and display correctly
- [ ] Comment UI is discoverable/clear on table elements

### Steps to Reproduce
1. Open prose-reader with a spec containing tables
2. Hover over/select a table entry/cell
3. Attempt to add a comment
4. *Result: Comment action unavailable or fails silently*

---

## Bug #2: Missing "Add Note" Discoverability on Desktop
**Severity:** P0 - Core feature not discoverable
**Area:** prose-reader desktop, note-taking UX

### Description
On desktop, there is no clear/visible way to discover how to trigger the 'add note' feature. The action may exist but users cannot find it without documentation.

### Impact
- Note-taking feature is invisible to users
- Low adoption of note functionality
- Desktop UX feels incomplete vs. expected features

### Root Cause
- No obvious UI button/affordance for "Add Note"
- No keyboard shortcut hint/documentation
- No onboarding/tutorial for this feature
- Possibly no right-click context menu option

### Acceptance Criteria
- [ ] "Add Note" action is discoverable on desktop (visible button/menu)
- [ ] Keyboard shortcut is displayed and documented
- [ ] Tooltip/help text explains the feature
- [ ] Works consistently across prose-reader

### Possible Solutions
- Add persistent "Add Note" button in toolbar or sidebar
- Add right-click context menu option: "Add Note"
- Display keyboard shortcut hint in UI (e.g., `Ctrl+Alt+N`)
- Add "?" help icon with feature discovery

---

## Next Steps
- [ ] Verify both bugs on current prose-reader build
- [ ] Prioritize UX fix for "Add Note" discoverability (higher user impact)
- [ ] Fix comment system to support table entries
- [ ] Add keyboard shortcut documentation to help system
- [ ] Consider UX audit of other hidden features
