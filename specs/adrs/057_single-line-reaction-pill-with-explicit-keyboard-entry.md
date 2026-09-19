# ADR-057: Single-line reaction pill with explicit keyboard entry

- **Status:** Accepted
- **Date:** 2026-09-19
- **Affects:** REQ-PF-018, REQ-PF-019, REQ-PF-020, REQ-KB-004

## Context

User testing of the inline reaction fixture favored an always-small input over a multiline bubble. The approved fixture also preserved unfinished reactions in a dock when their source scrolled away or unmounted. The user requested a keyboard path from native text selection into the reaction input.

## Options considered

- Retain the larger input from ADR-056 and require clicking it to type.
- Automatically focus the input on selection, sacrificing native selection interaction.
- Keep a single-line pill unfocused on selection and use explicit Enter to begin typing.

## Decision

Choose explicit keyboard entry. Unmodified Enter focuses an anchored pill only when another interactive control does not own the key and composition is not active. Cmd/Ctrl+Enter appends; plain Enter in the input does not append or submit. Long reactions scroll horizontally instead of expanding the input.

Keep unfinished text conversation-scoped outside virtualized rows. A docked preview returns to the source occurrence through production transcript navigation, reconstructs its selected range after mounting, and restores the pill. Manual return also restores it. Both presentations offer explicit dismissal with Keep/Discard confirmation.

This refines ADR-056's input presentation and click-only entry while preserving its no-autofocus, native-selection, draft ownership, and append semantics.

## Consequences

- The workflow supports selecting, pressing Enter, typing, and pressing Cmd/Ctrl+Enter without a pointer trip to the composer.
- Other editable controls, buttons, links, modified Enter, and IME composition retain their key behavior.
- The long production-component fixture and unmount/remount regression tests remain part of verification.
- Native phone keyboard and selection-menu acceptance still requires physical-device testing.
