# Render stable work-reference handles as compact interactive references

Coordinator responses currently render stable handles such as `@work:80ebbc46-67fe-41e7-9105-f8349d2764eb` as undifferentiated plain text. On mobile, UUID-length handles wrap across several lines, dominate the response, and make a list of cited work difficult to scan.

Investigate the markdown/message rendering boundary and add a compact, readable display for recognized `@work:`, `@conv:`, and `@chain:` references. Preserve the complete stable reference as authoritative data for copying, citation, and resolution; do not replace it with a lossy second representation. Prefer an actionable chip/link with a concise label or title when deterministically available, stable navigation, and an accessible full-reference name or copy affordance. Unknown or malformed text must remain ordinary text rather than becoming a misleading link.

## Acceptance criteria

- Recognized stable references in assistant messages are visually distinct, compact, and do not produce UUID-heavy multi-line walls on phone widths.
- Activating a reference navigates to or resolves the correct durable source; copying can recover the exact original handle.
- `@work`, `@conv`, and `@chain` forms have explicit tested behavior, including punctuation boundaries, malformed handles, archived/closed work, missing display metadata, and narrow responsive layouts.
- The implementation reuses the existing authoritative reference resolver and does not introduce parallel semantic representations or trust assistant-provided labels as authoritative metadata.
- Focused renderer tests and responsive fixture/browser coverage demonstrate readable grouped references without horizontal overflow.
