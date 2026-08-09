# Establish the iOS vNext rendering fixture harness

## Outcome

Provide a deterministic, background-oriented QA foundation for cataloging real native components and states as renderer work is split into repeatable leaf tasks.

## Dependencies

Blocked by the shipped ProductConversation client contract and the native migration seam. Fixtures must model the new aggregate, not preserve transcript-row assumptions.

## Scope

- Exercise the real SwiftUI conversation, state, tool, and Markdown components that exist when the base harness is built.
- Cover representative normal, loading, empty, malformed, error, offline, cached, and read-only states for those components.
- Keep deterministic data and timing so repeated captures are comparable.
- Extend the existing `ios/PhoenixMobile/UITests/` and focused test seams without taking over the Mac's mouse or keyboard.
- Define the fixture conventions that later renderer, grounding, and reader leaf tasks must extend alongside their components.

Before implementation, split this umbrella into narrow fixture/catalog tasks.

## Acceptance

- Every existing component family in scope has a deterministic inspection surface.
- Fixture output makes missing or generic rendering conspicuous, and feature leaf tasks can add fixtures without changing the base harness architecture.
- The harness runs without a live model and without foreground system control.
- CI-safe tests remain separate from the opt-in live-server journey.

## Out of scope

Implementing missing renderers or fixtures for components that do not yet exist; those fixtures ship with the corresponding feature leaf tasks.
