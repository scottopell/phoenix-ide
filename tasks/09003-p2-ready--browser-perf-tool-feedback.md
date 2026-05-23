# Browser performance tool feedback from React UI perf hunt

Raw observations/opinions from using `browser_profile` and the Phoenix perf skills during the React UI performance pass.

## What worked well

- The raw-sample model was very useful. Seeing every run made it possible to reject plausible-but-bad changes instead of trusting averages or intuition.
- Methodology warnings were valuable. The no-CPU-throttle warning caught an invalid load baseline before it became evidence.
- The scenario reset + readiness-window model was strong once the fixture existed. It made before/after runs comparable and forced us to separate app load from measured interaction.
- The saved JSON files in `/tmp` were helpful when tool output was large.
- React metrics and browser metrics in the same sample were useful. Some candidates moved React time, others moved script/wall time, and having both avoided over-indexing on one signal.
- The tool helped change behavior: we rejected several changes that looked structurally cheaper but did not prove out.

## Friction / sharp edges

- The saved scenario file shape surprised me. The tool message said raw samples were written, but the file contained the full response wrapper with `raw_samples`, while `stats.py` expects a raw array. Easy to work around, but it broke flow.
- Selector failures required manual diagnosis. A `wait_selector textarea` failure turned out to be a “Conversation not found” page, but the tool error did not include enough page context to see that immediately.
- The YAML scenario bridge was repetitive in the LLM context. Since `BROWSER_PROFILE_CMD` is not available in-shell here, I had to generate request JSON, manually call the tool, and manually extract samples.
- CPU profile summaries were useful, but idle dominated many summaries. I had to mentally ignore `(idle)` to find actionable work.
- It was easy to lose provenance between temp files, commits, scenarios, and candidates. I had to be disciplined about copying sample arrays and writing commit messages with the relevant numbers.
- Some scenario assumptions were stale: `conversation-load` referenced `fixture-turn-one`, but the seed path did not create it; `sse-streaming` assumed mock was enabled, but the dev env had it commented out.

## Overall opinion

The measurement model is solid and changed the outcome of the work in a good way. The ergonomics are still somewhat manual and require discipline, but the tool was good enough to make scientific yes/no calls. The most valuable part was not finding wins; it was confidently rejecting non-wins.
