# Render Markdown images in conversation messages

## Problem

Agents commonly include screenshots in responses using standard Markdown image syntax:

```md
![file-tree-dark-single-slot](ui/qa-artifacts/grounding-panel/file-tree-dark-single-slot.png)
```

Phoenix conversation Markdown currently renders links/tables/code/mermaid, but local image references in assistant messages do not resolve to a displayable image. Relative paths such as `ui/qa-artifacts/...png` need to be interpreted relative to the conversation/worktree root and served through Phoenix’s existing file allowlist/preview machinery rather than as browser-relative URLs.

## Scope

Add image support for conversation Markdown rendered in agent messages, including the example syntax above.

## Plan

1. Add a shared conversation Markdown image renderer beside `ConversationMarkdownAnchor`.
   - Preserve normal remote/data URLs when safe to render directly.
   - Resolve local absolute paths and root-relative/relative paths to Phoenix preview URLs, using the same root context used for clickable file paths.
   - Use existing `/preview/<absolute-path>` serving where possible so image bytes remain server-side and pass through the existing allowlist.
   - Keep `alt` text and make the image accessible.

2. Thread root context into the Markdown component maps.
   - Finalized `AgentMessage` already has `filePathRootDir`; use it for relative image resolution.
   - Ensure compact-expanded text, normal text, and sub-agent/result Markdown surfaces use a consistent component map where they have enough root context.
   - For streaming Markdown, either support only already-resolvable absolute/remote URLs or thread the current root if available without broad plumbing.

3. Add styling for inline conversation screenshots.
   - Constrain max width to the message column.
   - Preserve aspect ratio.
   - Add a subtle border/background suitable for light and dark themes.
   - Avoid layout-breaking full-size screenshots.

4. Add tests.
   - Finalized assistant Markdown image with `ui/qa-artifacts/example.png` renders an `<img>` whose `src` is a Phoenix preview URL under the conversation root.
   - Remote `https://...` image sources pass through unchanged.
   - Existing Markdown links still open in a new tab with safe `rel` attributes.
   - Existing table/code/mermaid behavior remains covered or unchanged.

## Acceptance criteria

- The Markdown snippet `![file-tree-dark-single-slot](ui/qa-artifacts/grounding-panel/file-tree-dark-single-slot.png)` displays the image in an assistant message when the file exists under the conversation root.
- The implementation does not introduce arbitrary host-file reads; local images are only served through existing Phoenix file/preview allowlist paths.
- Broken or disallowed local image paths degrade to a normal broken image/alt display rather than crashing message rendering.
- `./dev.py check` passes.
