# Open conversation Markdown links in new tabs

## Problem

Markdown links rendered inside conversation content currently use the browser default navigation behavior, so clicking a PR / issue / docs link can navigate the Phoenix conversation tab in-place. This is especially painful when Phoenix is pinned and used as a persistent workspace.

Plain auto-linkified URLs already open in a new tab; Markdown links should behave the same way.

## Plan

1. Add a shared Markdown anchor renderer for conversation/message surfaces that renders links with:
   - `target="_blank"`
   - `rel="noopener noreferrer"`
2. Wire that renderer into the Markdown component maps used by:
   - finalized agent message text
   - compact expanded text
   - streaming agent message text
   - sub-agent transcript / result previews where Markdown is rendered inside conversation content
3. Keep internal Phoenix navigation links outside conversation prose unchanged unless they are part of message Markdown.
4. Add/adjust UI tests to verify Markdown links in finalized and streaming conversation messages open in a new tab and retain safe `rel` attributes.
5. Run the relevant UI tests/checks via `./dev.py`.

## Acceptance criteria

- Clicking a Markdown PR link in an agent response opens a new browser tab by default.
- Plain URL auto-links continue to open in a new tab.
- Internal app links such as “Continue there” keep their current in-place behavior.
- Tests cover the Markdown link behavior.
