# Restore in-repo Markdown links in conversation messages

## Observed journey

- An agent includes a repository path in a conversation message, either as raw text such as `./specs/AUTHORING.md` or as a Markdown link such as `[AUTHORING.md](./specs/AUTHORING.md)`.
- Both forms should recognize the same local-file destination and open it in Phoenix's existing side-panel prose viewer. Raw paths still work; Markdown-encoded paths have regressed.
- Instead, the rendered anchor has `target="_blank"`; the browser resolves the relative href against the Phoenix web route and opens a broken URL such as `<phoenix-url>/specs/AUTHORING.md` in a new tab.
- The report concerns the conversation UI. The failure is directly reproducible from the current render path for finalized agent messages; the same shared anchor is also used by streaming and nested conversation Markdown surfaces.

## Verified findings

- `ConversationMarkdownAnchor` in `ui/src/components/conversationMarkdown.tsx` unconditionally renders every Markdown href with `target="_blank"` and does not receive or invoke the file-viewer callback.
- `AgentMessageImpl` in `ui/src/components/MessageComponents.tsx` already receives `onOpenFile` and resolves plain-text paths through it, but its Markdown component map assigns `a: ConversationMarkdownAnchor` without passing that callback.
- `handleOpenFileFromPatch` in `ui/src/pages/ConversationPage.tsx` is the established boundary that resolves relative paths against `conversation.worktree_path ?? conversation.cwd` and opens the prose viewer via `fileExplorer.openFile`.
- Existing tests under `conversation markdown links` verify new-tab behavior only for HTTPS links. They do not cover relative Markdown file hrefs.
- Git history identifies `bbb3324a9` (`Open conversation Markdown links in new tabs (#271)`) as the change that introduced the unconditional shared anchor. Its goal and tests addressed external links, but the implementation did not preserve the pre-existing local-file interaction.
- `linkifyText` in `ui/src/utils/linkify.tsx` already distinguishes recognized file paths from HTTP(S) URLs for unformatted prose and supports `./file.ext`, project-relative paths such as `specs/AUTHORING.md`, and absolute file paths.
- `MessageComponents.test.tsx` has regression coverage for a raw project-relative path (`src/main.rs`) opening through `onOpenFile`, but does not cover the raw `./path` form from this report.
- No normative requirement explicitly defines which local-reference encodings in agent prose are interactive. `specs/file-explorer/requirements.md` line 71 says linkified conversation paths share the server's viewer-openability verdict with other file entry points, but it does not define raw-path versus Markdown-link recognition or their click destination. This contract is therefore under-specified.

## Inferences and unknowns

- The failure model is an over-broad anchor policy, not a backend file-read or viewer-slot failure: the click never reaches the existing `onOpenFile` boundary.
- The intended distinction can be made from the href before browser navigation. Raw and Markdown-encoded references should reuse one local-path classifier rather than maintain separate extension/path grammars that can drift.
- No product decision is needed for the reported case. URL-like hrefs (HTTP(S), mailto, and other safe external schemes) should retain ordinary anchor behavior; repository-file hrefs should use the viewer. Fragment-only links and non-file relative web paths are not part of the reported behavior and must not be accidentally treated as files.

## Interaction map

- Agent Markdown text → `ReactMarkdown` parses an anchor → conversation Markdown anchor policy classifies its href.
- External URL → safe `<a target="_blank" rel="noopener noreferrer">` behavior remains unchanged.
- Recognized local file href → prevent browser navigation → `onOpenFile(path, empty modified-line context, 0)` → `ConversationPage.handleOpenFileFromPatch` resolves against the conversation worktree/cwd → `FileExplorerContext` / viewer slot opens the prose side panel.
- This is presentation and navigation state only. It introduces no persistence, SSE wire, recovery, cancellation, or backend API changes.

## Proposed scope

### Owning invariant

A conversation Markdown link that denotes a local repository file must use Phoenix's file-viewer navigation, while a genuine external link must remain a safe browser link. Formatting a path as Markdown must not change its destination from the in-app viewer to the current web origin.

The conversation-rendering contract must explicitly cover both supported encodings: a recognized raw local path and a Markdown link whose href is that same local path have identical file-viewer behavior. The Markdown label is presentation only; the href is the path authority.

### Implementation

1. Add a timeless requirement to `specs/conversation-ui/requirements.md` defining agent-message local references: recognized raw local paths and Markdown links with recognized local-path hrefs open the same conversation file viewer; external URL links retain browser-link behavior. Cross-reference the existing single server-side viewer-openability authority in `specs/file-explorer/requirements.md` rather than duplicating its text/image/binary classification.
2. Extract or expose one local-path candidate classifier from `ui/src/utils/linkify.tsx`, preserving the currently supported raw shapes (`./file.ext`, `../dir/file.ext`, project-relative multi-segment paths with recognized extensions, and absolute multi-segment paths). Use it for both raw text and Markdown href handling. Do not make frontend path syntax detection a second authority for whether file contents are actually viewer-openable; that remains server-owned.
3. Refactor the conversation Markdown anchor/component factory so it can receive the conversation's file-open callback and classify hrefs with that shared local-path classifier.
4. For recognized local file hrefs, render an accessible file-path control (or intercept the anchor click and keyboard activation) that invokes `onOpenFile` and does not expose a browser-resolvable relative `href`/new-tab target. Preserve the visible Markdown label while retaining path metadata needed by existing file-path context-menu/copy behavior where applicable.
5. Keep external URL anchors opening in a new tab with `noopener noreferrer`.
6. Wire the callback into finalized agent Markdown. Also keep streaming/finalized behavior consistent by threading the existing file-open capability to streaming Markdown rather than allowing the destination to change when a message finalizes. If nested/read-only transcript surfaces intentionally lack a viewer callback, render local file references as non-navigating path text instead of broken browser links.
7. Avoid broadening local-path classification to fragment links, arbitrary SPA-relative URLs, or unsafe URI schemes.
8. Update `specs/conversation-ui/executive.md` with the resulting current reality and verification anchor, following the spec authoring pre-flight checklist.

Likely starting symbols:

- `ConversationMarkdownAnchor` / conversation Markdown component creation in `ui/src/components/conversationMarkdown.tsx` and `conversationMarkdownImages.ts`
- `parseLinks` / local path classification in `ui/src/utils/linkify.tsx`
- `AgentMessageImpl.markdownComponents` in `ui/src/components/MessageComponents.tsx`
- `StreamingMessage` and its call site in `ui/src/components/MessageList.tsx`
- Existing `handleOpenFileFromPatch` in `ui/src/pages/ConversationPage.tsx` (reuse, not redesign)

### Regression and journey validation

Add focused UI tests proving:

- A raw `./specs/AUTHORING.md` in a finalized agent message remains linkified and calls `onOpenFile` with that exact path.
- Clicking `[AUTHORING.md](./specs/AUTHORING.md)` in a finalized agent message calls `onOpenFile` with `./specs/AUTHORING.md` and does not navigate/open a new tab.
- A project-relative Markdown href such as `specs/AUTHORING.md` follows the same viewer path.
- An HTTPS Markdown link still has its original href, `_blank` target, and safe rel attributes.
- Fragment-only and non-file relative hrefs are not misclassified as repository files.
- Streaming and finalized rendering agree for local Markdown file links, so finalization does not change click behavior.
- Run the relevant UI tests and the repository's gated `./dev.py check` validation.

Manual acceptance journey: open a conversation rooted in this repository and click both a raw `./specs/AUTHORING.md` reference and `[AUTHORING.md](./specs/AUTHORING.md)` in agent prose. Verify both open the same file in the side panel while the browser remains on the conversation route and no tab is created.

## Risks and non-goals

- Risk: duplicating the path grammar would let plain-text and Markdown paths diverge; share the classifier.
- Risk: an anchor with a friendly label has a different displayed string from its href, so classification must use the href while preserving the label.
- Risk: `../` paths or absolute paths can cross the repository boundary. Preserve existing file-viewer authorization/canonicalization and do not weaken backend path policy; the acceptance case is an in-repo relative path.
- Non-goals: changing the file viewer, URL-driven viewer-slot persistence, Markdown image handling, server file APIs, external-link policy, or generic application routing.
