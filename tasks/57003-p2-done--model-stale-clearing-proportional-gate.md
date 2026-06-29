# Model stale tool-result clearing with a proportional gain gate

## Goal

Create a small, standalone modeling artifact that makes the stale tool-result clearing algorithm intuitive before changing runtime behavior.

The artifact should let us compare the current absolute `CLEAR_AT_LEAST_TOKENS` gate against a proposed proportional gate:

```text
min_required_gain = max(CLEAR_AT_LEAST_TOKENS, current_prompt_tokens * gain_fraction)
```

The primary question: when a long patch-heavy session reaches the ratchet state where a sweep frees ~8k tokens but invalidates a ~190k-token prefix, does the proportional gate correctly hold the watermark instead of causing repeated low-yield cache-busting sweeps?

## Deliverable

Add a single-page HTML simulator artifact, preferably under a discoverable docs/experiments or docs/qa location, with no build step and no dependency on the Phoenix runtime.

The simulator should include:

1. A simplified model of the current planner:
   - context window
   - clear trigger fraction
   - keep-recent-rounds floor
   - monotonic clear watermark
   - clearable vs unclearable token accumulation
   - eligible freed tokens below the recency floor
   - current absolute gain gate

2. A proportional-gate policy for comparison:
   - `max(absolute_min_gain, prompt_tokens * gain_fraction)`
   - configurable gain fraction

3. Scenario presets:
   - short clean one-sweep win
   - patch-heavy ratchet
   - read/search-heavy clearable session
   - mixed long coding session
   - approximate prod case-study shape for `add-phoenix-native-commission-review-tool`

4. Interactive controls:
   - context window
   - trigger fraction
   - keep recent rounds
   - absolute minimum gain
   - proportional gain fraction
   - clearable/unclearable token rates or ratios
   - session length / tool density
   - optional cache-recovery knobs for rough billing intuition

5. Visual output:
   - timeline of prompt size / retained tokens / cleared tokens
   - trigger and context-window reference lines
   - sweep markers
   - side-by-side current-vs-proportional comparison
   - sweep table with turn, pre-prompt, post-prompt, freed tokens, required gain, and decision

6. Notes in the artifact explaining assumptions and limitations:
   - this is a planner intuition model, not an exact replay of provider billing
   - OpenAI cache cost is approximated because prod records usage outcomes, not internal prefix-cache identities
   - provider asymmetry should be visible conceptually but not treated as exact pricing

## Acceptance criteria

- Opening the HTML file in a browser shows the simulator without running Phoenix.
- The patch-heavy ratchet preset reproduces the important qualitative behavior: current policy performs repeated low-yield sweeps, while a reasonable proportional threshold holds the watermark once freed tokens are too small relative to prompt size.
- The short clean-win preset still allows a useful sweep under the proportional policy.
- The artifact makes policy tradeoffs visible enough to guide the subsequent runtime fix.
- No production runtime behavior changes are included in this task.

## Follow-up, out of scope

After the model is reviewed, implement the runtime planner change and add unit tests around the proportional gate and ratchet prevention.
