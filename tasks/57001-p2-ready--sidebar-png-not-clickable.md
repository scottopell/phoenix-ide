PNG files are not rendered as clickable in the file-tree sidebar, but they ARE openable in the MetaViewer — selecting one via cmd-p (quick open) or clicking a linkified path in a conversation opens the image in the MetaViewer correctly. The sidebar click affordance is the gap: image file types (png and other MetaViewer-supported images) should be clickable in the sidebar the same way they are reachable via cmd-p / conversation links.

Fix: make the sidebar treat MetaViewer-supported image types as clickable/openable, consistent with the cmd-p and linkify paths.

Relevant: ui/src/components/FileViewer.tsx (image payload), ui/src/components/viewer/MetaViewer.tsx, the file-tree sidebar component, ui/src/utils/linkify.tsx.
