# Establish the iOS vNext rendering fixture harness

## Outcome

Provide a deterministic, background-oriented QA surface that catalogs the real native components and states before renderer work is split into repeatable leaf tasks.

## Dependencies

Blocked by the shipped ProductConversation client contract and the native migration seam. Fixtures must model the new aggregate, not preserve transcript-row assumptions.

## Scope

- Exercise real SwiftUI conversation, state, tool, Markdown, grounding, and reader components.
- Cover representative normal, loading, empty, malformed, error, offline, cached, and read-only states.
- Keep deterministic data and timing so repeated captures are comparable.
- Extend the existing `ios/PhoenixMobile/UITests/` and focused test seams without taking over the Mac's mouse or keyboard.

Before implementation, split this umbrella into narrow fixture/catalog tasks.

## Acceptance

- Every iOS vNext section has a deterministic inspection surface.
- Fixture output makes missing or generic rendering conspicuous.
- The harness runs without a live model and without foreground system control.
- CI-safe tests remain separate from the opt-in live-server journey.

## Out of scope

Implementing the missing renderers themselves.
