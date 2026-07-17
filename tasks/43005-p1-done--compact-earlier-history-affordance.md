# Make earlier-history loading a compact transcript affordance

Long conversations intentionally load the newest history slice first, but MessageList renders the explicit earlier-history action using the global secondary-button treatment. This makes pagination visually dominate the conversation despite being a low-frequency progressive-disclosure action.

Replace it with a compact, centered transcript-owned control while preserving loading, retry, accessibility, and history-continuity behavior. Add focused tests for normal, loading, and retry states.
