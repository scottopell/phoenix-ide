# Split review notes data subscriptions

Task 51001 validated candidate C6 as a likely broad-context broadcast: file and diff note consumers subscribe to the entire `ReviewNotesContext`, so every note mutation changes the provider value for all consumers and causes unrelated scopes to refilter.

Evidence from the 51001 audit:
- `ui/src/contexts/ReviewNotesContext.tsx` exposes one context containing the full `notes` pile and all commands.
- `ui/src/components/viewer/useFileReviewNotes.ts` and `useDiffReviewNotes.ts` derive filtered data from that full pile.

Acceptance criteria:
- Add render/filter-count instrumentation for a file-note consumer and a diff-note consumer.
- Mutate notes in one scope and verify the unrelated scope currently re-renders/refilters.
- Split command access from data subscriptions, or add typed selector hooks/stores such that file note consumers subscribe only to their file path and diff note consumers subscribe only to diff notes.
- Preserve the single authoritative notes pile for send/export behavior; do not introduce parallel state.
