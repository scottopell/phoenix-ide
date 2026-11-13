# ADR-229: Reaction pills grow with wrapped text

- **Status:** Accepted
- **Date:** 2026-11-13
- **Affects:** REQ-PF-018, REQ-PF-019, REQ-PF-020, REQ-KB-004

## Context

ADR-057 keeps reactions one line tall and scrolls long text horizontally. During fixture iteration, the user requested that the pill expand as soon as text exceeds its visible width so the reaction remains readable while composing.

## Options considered

1. Retain ADR-057's fixed-height input: compact, but hides the beginning of long reactions while typing.
2. Start compact and grow a wrapping editor with its content: keeps short reactions small and long reactions readable, but requires placement to follow editor height.

## Decision

Choose an initially single-line textarea that wraps and grows automatically, and shrinks after text is removed. Bound its height and scroll vertically for very long reactions. Use native content sizing, as in the question panel, with the existing pill resize observer keeping its placement aligned.

Preserve ADR-057's interaction contract: selection does not autofocus; unmodified Enter outside other controls focuses the editor; Cmd/Ctrl+Enter appends; plain Enter inside the editor does not append or submit. Keep source identity and unfinished text conversation-scoped, and retain source-return navigation and explicit Keep/Discard dismissal.

This supersedes ADR-057's fixed-height presentation and retains its keyboard and ownership decisions.

## Consequences

- **Positive:** Long reactions remain readable without horizontal scrolling, while short reactions retain the compact pill.
- **Negative:** Growing reactions cover more transcript space; height limits and resize-aware placement remain necessary.
- **Neutral:** Native text selection, draft append, and temporary reaction ownership keep their existing contracts.

## References

- [ADR-057](057_single-line-reaction-pill-with-explicit-keyboard-entry.md)
- [Prose feedback requirements](../prose-feedback/requirements.md)
- `ReactionPill`, `InlineMessageReaction`, and `InlineReactionStore`
