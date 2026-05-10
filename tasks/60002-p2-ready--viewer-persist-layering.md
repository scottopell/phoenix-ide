---
created: 2026-05-10
priority: p2
status: ready
artifact: ui/src/components/FileExplorer/lastViewerStorage.ts
---

lastViewerStorage.ts lives under ui/src/components/FileExplorer/ but is now imported by ui/src/conversation/useConversationsRefresh.ts (REQ-VS-014 hard-delete cascade clears the entry). conversation/ is structurally more foundational than components/, so this is backwards layering.

Two clean options:

1. Move ui/src/components/FileExplorer/lastViewerStorage.ts to a neutral location such as ui/src/storage/lastViewerStorage.ts. Update the two import sites (FileExplorerContext.tsx, useConversationsRefresh.ts) and any test imports.

2. Keep the file co-located with FileExplorerProvider (its primary owner), but invert the dependency: FileExplorerProvider registers a clear callback with ConversationStore (or via a small subscription), and useConversationsRefresh.ts calls store.onHardDelete callbacks instead of importing storage helpers directly.

Option 1 is simpler and keeps the helper string-in / string-out (no React, no store coupling). Recommend that unless we want a more general hard-delete-cascade hook system.

Acceptance:
- ui/src/conversation/* does not import from ui/src/components/*
- All existing REQ-VS-014 tests still pass
- ./dev.py check green
