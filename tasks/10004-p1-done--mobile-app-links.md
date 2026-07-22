# Make Coordinator conversation and message links navigate in-app on mobile

Coordinator citations already contain valid app-relative `/c/<slug>` and `#message-<id>` destinations, but the shared conversation Markdown anchor forces every non-file link through `target="_blank"`. Mobile installed/standalone browser contexts can ignore or fail that new-context navigation, leaving taps with no visible result.

## Scope

- Classify same-origin/app-relative Markdown destinations separately from external URLs.
- Navigate app-local conversation/message citations in the current Phoenix browsing context while preserving external links in a safe new tab and local file-path viewer behavior.
- Preserve message fragments so a cited rendered message remains the destination.
- Add focused regression coverage for app-relative, same-origin absolute, external, and file-path anchors, including the mobile-relevant no-new-context contract.
- Validate the Coordinator citation journey at a mobile viewport.

## Acceptance evidence

- Tapping `/c/<slug>` or `/c/<slug>#message-<id>` from Coordinator chat changes the current Phoenix route rather than requesting a new browser tab/window.
- External HTTPS links still use `target="_blank"` with safe rel attributes.
- File path links still open through the existing Phoenix file viewer path.
- Focused UI tests and relevant checks pass.

## Non-goals

- Redesigning citation text or compact `@conv`/`@work` handles.
- Changing Coordinator SQL/search/reference production.
- Adding click behavior to plain-text `@conv` handles.
