# Add desktop and mobile QA fixtures for `/new`

## Goal

Add a deterministic Ladle fixture for the new-conversation page and expose it through `./dev.py qa`, producing comparable desktop and mobile screenshots for layout work.

## Plan

1. Add a `newConversation` fixture surface under `ui/src/fixtures/` with typed scenario data, deterministic local-storage setup, and mocked API responses for a fully loaded, LLM-configured Git project. Render the real `NewConversationPage` with its required router and conversation providers rather than duplicating page markup.
2. Give the rendered fixture a stable scenario-valued ready marker only after model, directory, branch, and task metadata have settled, so capture does not depend on sleeps or live backend state. Restore mocked APIs and browser state on cleanup to prevent Ladle story leakage.
3. Add a Ladle story for the representative ready state. Keep the scenario list as the source of truth for story and capture discovery.
4. Add a `capture-new-conversation.mjs` entry using the shared `runSurfaceCapture` engine with desktop `1280×900` and mobile `390×844` viewports. Screenshots should be written beneath `ui/qa-artifacts/new-conversation/` with viewport suffixes.
5. Wire the surface into `ui/package.json` and `./dev.py qa new-conversation`, including help text that identifies desktop/mobile capture.
6. Add focused fixture tests proving that the real page reaches its ready state with representative directory, model, workflow, and composer content, and that the ready marker is present. Reuse the shared viewport-matrix behavior rather than introducing another capture implementation.
7. Validate with the focused UI tests, TypeScript checks, `./dev.py qa new-conversation`, `./dev.py qa --help`, and `./dev.py check`.

## Acceptance criteria

- `./dev.py qa new-conversation` captures the same deterministic `/new` scenario at desktop and mobile sizes.
- The fixture renders the production `NewConversationPage` and production responsive CSS.
- Capture uses a stable settled-DOM marker and makes no live Phoenix API calls.
- Generated artifacts clearly distinguish desktop and mobile screenshots.
- Targeted tests and the repository check pass.
