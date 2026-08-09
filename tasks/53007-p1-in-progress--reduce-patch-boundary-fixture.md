# Reduce oversized patch boundary fixture

Replace the 2 MiB allocation used to prove patch candidate-search size limits with a small pure boundary test while preserving the production content and anchor limits. Capture focused before/after CPU and wall measurements, demonstrate deliberate faults for strictness and both guarded inputs, run focused and broad validation, and land as a separate PR.
