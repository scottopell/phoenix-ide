Codex backend supports a 1M-token max for gpt-5.4 and codex-auto-review via opt-in (`max_context_window: 1000000` in `~/.codex/models_cache.json`). Phoenix declares 272K default for all codex-backed models — matches the default cap, but leaves the higher ceiling on the table for users who could use it.

Investigation needed:
- How does codex CLI request the higher ceiling? Specific header? Different api_name? Per-request param?
- Is the opt-in account/plan-gated?
- Does the cost model differ?

If trivial (e.g. a single header), wire it up behind a per-model flag so eligible models opt in automatically. If account-gated, expose as a toggle in conversation settings.

For now, every Phoenix user gets 272K on all codex models. That matches what worked previously (before the 1M marketing claim in models.rs was discovered to be a lie) and is safer than the silently broken 1M declaration.

Discovered 2026-05-11 during root-cause of context_length_exceeded errors. See sibling task for the bug fix.
