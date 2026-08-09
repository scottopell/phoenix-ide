# Reduce oversized patch boundary fixture

Replace the 2 MiB allocation used to prove patch candidate-search size limits with a small pure boundary test while preserving the production content and anchor limits. Capture focused before/after CPU and wall measurements, demonstrate deliberate faults for strictness and both guarded inputs, run focused and broad validation, and land as a separate PR.


## Evidence

Fresh-main baseline from `target/check-profile/fresh-main-warmed-6d399e676-20260809-100328`: `oversized_inputs_skip_candidate_search` used 1,344.512 ms CPU and 1,666.379 ms wall. Candidate profile `target/check-profile/53007-after-20260809-105326` measured 6.365 ms CPU and 32.244 ms wall, saving 1,338.147 ms CPU (99.53%) and 1,634.135 ms wall (98.07%).

Focused validation ran the renamed boundary test (1/1 passed). Deliberate mutants changing the file limit from strict `>` to inclusive `>=`, and removing the anchor limit, each failed at the intended assertion; logs are under `target/qa-evidence/53007/`. The profiled broad check completed Rust tests but had one unrelated Vitest failure in `ConversationPage.archived.test.tsx` (`history-has-older` expected `yes`, received `no`); 2,156 other Vitest cases passed.
