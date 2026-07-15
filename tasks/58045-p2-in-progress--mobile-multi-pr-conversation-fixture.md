# Capture the mobile conversation UI with multiple open PRs

## Goal

Create a deterministic `./dev.py qa`-style fixture that renders the real conversation UI when two pull requests are associated with one conversation, then use it to capture and review the current mobile experience as the baseline for follow-up UI work.

## Scope

- Inspect the multi-PR conversation flow, its normative specs, and the existing Ladle/QA fixture conventions.
- Add a representative fixture scenario with two actionable open PRs, including enough realistic metadata to exercise active-PR selection and the surrounding conversation controls.
- Render the production conversation components rather than a visual mock, with stable fixture-ready markers and no live network or timing dependencies.
- Add or extend a Ladle story and `./dev.py qa` capture route using the shared capture harness.
- Capture at a phone-sized viewport, including the initial state and any essential PR-selection state needed to expose the mobile interaction.
- Add targeted fixture/component tests that ensure both PRs reach the rendered UI and the intended selection state remains representable.
- Run focused UI checks and the new QA capture, then present the screenshots and a concise assessment of the mobile layout as the starting point for a separate redesign decision.

## Boundaries

- This task establishes an accurate, repeatable visual baseline; it does not redesign the multi-PR UI yet.
- Fixture-only styling must not mask production layout problems.
- Existing multi-PR behavior and selection semantics remain unchanged unless fixture integration exposes a concrete correctness bug required to render the state.

## Acceptance criteria

- `./dev.py qa --help` exposes the relevant conversation/multi-PR capture surface.
- A deterministic Ladle scenario renders a conversation with exactly two open associated PRs through production UI components.
- The QA command produces stable mobile screenshots without arbitrary sleeps.
- Screenshots clearly show the current mobile UI and, where applicable, the opened PR chooser.
- Targeted tests and type checking pass, and the resulting visual/UX observations are reported to the user.
