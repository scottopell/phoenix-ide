# Trim tool-results renderer fixtures

Reduce payload and render work in the broad ToolResults fixture tests while preserving every specialized renderer family, fallback state, truncation boundary, and full-shell no-virtualization contract. Capture focused before/after Vitest CPU and wall measurements, deliberate fault proof, focused/full validation, and a separate PR.


## Evidence

Fresh-main profiles measured the grouped specialized renderer test at 369.077 and 389.873 ms CPU, and the full-shell test at 187.392 and 185.691 ms CPU. Five candidate samples measured medians of 308.643 ms CPU / 124.968 ms wall for grouped renderers and 159.144 ms CPU / 106.511 ms wall for full shell, saving about 18.7% and 14.7% CPU against the two-run baseline medians.

The fixture keeps the exact 21-line boundary required to overflow the 20-line preview, retains all three typed/display/legacy image paths with valid 1×1 SVG bytes, and keeps every family/fallback assertion. Deliberate faults reducing the shell fixture to 20 lines and removing the legacy image path each failed at the intended render assertion; logs are under `target/qa-evidence/53008/`. An exploratory 21-long-line shape was rejected because it crossed the character cap first and changed the tested contract.
