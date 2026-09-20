# ADR-059: Touch reactions dock away from native selection

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** REQ-PF-018, REQ-PF-020

## Context

Native iPhone selection presents a system menu beside the selected passage. The floating reaction pill approved in ADR-057 competes for the same space, while Phoenix cannot measure or control the system menu. The existing reaction dock, source identity, range restoration, and visual viewport handling can be extended to the app-owned composer geometry without introducing another interaction surface.

## Options considered

1. **Keep selection-relative placement on every device** — preserves one presentation, but overlaps native touch selection UI.
2. **Estimate the native menu bounds or raise the pill above it** — keeps the pill near the passage, but relies on browser-specific offsets and competes with system UI Phoenix does not own.
3. **Dock touch reactions above the composer** — separates app and system controls while retaining the existing pill, source-return behavior, and draft ownership.

## Decision

Use selection-relative floating placement for fine-pointer and keyboard selection. For touch or coarse-pointer selection, immediately use the existing `ReactionPill` as an unfocused dock above the current composer. The dock shows a short source preview and the same one-line editor and actions. It captures the exact `ReactionSource` and native range before deliberate input focus can clear native selection, and follows visual-viewport and composer geometry when the software keyboard opens.

The dock does not suppress selection, context menus, handles, or scrolling. It does not estimate native-menu geometry or introduce another reaction or draft representation.

## Consequences

- Native selection UI and the app reaction control occupy separate regions.
- Desktop behavior remains unchanged.
- The touch dock must observe both composer and visual-viewport geometry.
- Physical-device qualification remains necessary because browser emulation cannot establish native-menu coexistence.

## References

- ADR-056, ADR-057
- `ReactionPill`, `ReactionSession`
- `specs/prose-feedback/requirements.md`
