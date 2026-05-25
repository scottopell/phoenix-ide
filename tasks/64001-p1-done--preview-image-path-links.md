# Preview image files opened from conversation path links

## Problem

Clicking a screenshot path that an agent writes in a conversation currently routes the path through the same ProseReader used for text review. ProseReader always calls `/api/files/read`, which validates UTF-8 text. PNG/JPEG/etc. therefore fail with:

> File appears to be binary or has invalid encoding

That is technically consistent with the prose-feedback spec for text-file review, but it is wrong for this actual UX: a conversation path link to a screenshot should open an image preview, not a text reader error.

## Current findings

- `ui/src/utils/linkify.tsx` linkifies file paths and invokes `onFileClick`.
- `ui/src/components/MessageComponents.tsx` wires path clicks to `onOpenFile(filePath, new Set(), 0)`.
- `ui/src/pages/ConversationPage.tsx` resolves that path and calls `fileExplorer.openFile(...)`.
- `FileExplorerContext` only stores `ProseReaderState` (`?file=...&root=...`).
- `ProseReader` always fetches `/api/files/read?path=...` and expects text.
- Backend `/api/files/read` is generically named but currently has a text-only response contract: `ReadFileResponse { content: String, encoding: String }`. Existing consumers assume `content` is UTF-8 text.
- Backend `/preview/*filepath` already serves bytes with native `Content-Type`, and can either remain the binary transport or be referenced from a richer `/api/files/read` response.
- Backend file listing already classifies common image extensions as `file_type: "image", is_text_file: false`, but conversation links bypass file-tree disabled behavior.

## Desired behavior

When a user clicks a file path in a conversation and the target is an image (`png`, `jpg`, `jpeg`, `gif`, `webp`, likely `svg`, `bmp`, `ico`):

1. Open the viewer slot without showing the binary/encoding error.
2. Render the image using the existing backend preview route or an equivalent safe binary route.
3. Preserve close behavior, desktop split-pane behavior, mobile overlay behavior, and URL-addressable viewer state as much as practical.
4. Keep text files on the existing ProseReader path.
5. Avoid adding annotation/review-note affordances to images unless explicitly designed; this task is preview-only.
6. Add regression coverage for image file reads/routing/rendering so PNG links do not fall through to the current text-only `/api/files/read` behavior.

## Suggested implementation plan

- Evolve `/api/files/read` from a text-only response into an explicit typed file-read contract, while preserving compatibility for existing text callers. For example:
  - text files: `{ kind: 'text', content, encoding, file_type }` (or a backward-compatible superset that keeps `content` at top level);
  - image files: `{ kind: 'image', mime_type, url }` where `url` can point at `/preview/<absolute path>` rather than embedding image bytes in JSON.
- Update frontend read helpers/viewer routing to branch on this typed response instead of assuming every successful read is UTF-8 text.
- Extend the single viewer slot model to support either text/prose or image preview, rather than treating every `?file` as prose content.
  - A minimal option: ProseReader detects image extensions before calling `/api/files/read` and renders `<img src={`/preview${absolutePath}`}>` with appropriate chrome.
  - A cleaner option: introduce an `ImagePreview` viewer component and a typed viewer state (`kind: 'prose' | 'image'`) while preserving existing URL params/backward compatibility.
- Ensure relative conversation links still resolve against `conversation.worktree_path ?? conversation.cwd`.
- Add tests around:
  - path link with `.png` invokes image preview path, not text read;
  - image preview uses the typed `/api/files/read` image response and renders its preview URL;
  - text files still use ProseReader/read endpoint.

## Acceptance criteria

- Clicking an agent-written screenshot path ending in `.png` displays the image in the UI.
- No `File appears to be binary or has invalid encoding` error appears for supported image files.
- Text-file viewing and patch-file review behavior remain unchanged.
- `./dev.py check` passes.
