# Make Coordinator freshness discoverable and improve mobile UX

Add one safe, user-triggered “Brief me on current work” quick action that submits a normal read-only Coordinator turn, plus a quiet indicator that current deterministic work context is attached to each Coordinator message.

Improve the compact `/global` experience without expanding Coordinator authority: make Conversation/Work switching obvious and thumb-friendly, preserve useful context while switching, and keep the composer/briefing action reachable without crowding the transcript.

## Acceptance criteria

- A visible “Brief me on current work” action submits a normal durable Coordinator message asking for a succinct current-state briefing and explicitly forbidding messages or mutations.
- The UI explains that fresh current-work context is attached on each Coordinator turn without implying transcript freshness, background monitoring, or exact deltas.
- Compact layouts make Conversation versus Work navigation obvious, preserve mounted conversation state, and keep primary chat actions easy to reach.
- Existing desktop density, deterministic Work utility, normal composer semantics, and capability boundaries remain unchanged.
- Focused component and responsive browser coverage passes.
