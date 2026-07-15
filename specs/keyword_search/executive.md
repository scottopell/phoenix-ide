# Keyword Search Tool - Executive Summary

## Requirements Summary

Keyword search enables conceptual code search when agents lack precise information. It uses a two-stage approach: ripgrep searches for provided terms, then an LLM filters results for relevance. Search runs from git repository root (or working directory fallback) with case-insensitive matching and 10 lines of context. The search scope is respected even when it is a large, intentionally broad tree (e.g. a multi-repo working directory); breadth is made affordable rather than refused. Overly broad terms are dropped by a cheap early-exit match-count probe, and combined output is bounded by an always-on cap. The filtering LLM is selected from fast, cheap models to keep latency and cost acceptable.

## Technical Summary

Tool accepts query string and ordered search terms array. Scope resolution walks up from the working directory to the enclosing git root, falling back to the working directory; the filesystem root is refused as a floor. Each term is first probed with `rg --count-matches --max-count=<BROAD_TERM_MATCH_LIMIT + 1>`: its match total is accumulated as `rg` streams, and the child is killed the instant the total crosses `BROAD_TERM_MATCH_LIMIT`, so rejecting a broad term costs O(limit) regardless of tree size. `--count-matches` (not `--count`) counts individual matches so a term repeated many times on a single long line (a minified bundle) still reads as broad. Files are deliberately not size-capped, so the search never silently omits a file it was asked to cover; a large file is read in full but bounded by its size. Terms that probe to zero matches are excluded (they add nothing to the combined scan), an all-zero/absent term set returns "no matches" without a second full-tree scan, and a term set where every probe failed (e.g. ripgrep missing) returns an error rather than a false empty result. The per-file `--max-count` both bounds each file's scan and lets a term dense in a single giant file (a generated bundle, a huge log) trip the limit on that one file rather than reporting a small count and being wrongly accepted as narrow. A term whose probe exits with an error status (e.g. an invalid regex) is skipped individually rather than failing the whole search. Usable terms are then scanned with `rg -C 10 -i --line-number --with-filename -e <term>`, streaming stdout into a buffer killed at `MAX_COMBINED_RESULTS` (128KB) — the always-on ceiling that fires even inside a legitimate single repo. If the combined output overruns the ceiling, the lowest-priority term is dropped and the scan retried (terms are ordered by importance, REQ-KWS-004), so the byte budget favours the most important terms instead of filling in filesystem-traversal order. The final output carries a marker when it was truncated or when terms were dropped. Every `rg` child is raced against the cancellation token and killed+reaped on cancel (REQ-BED-005). Results plus query are sent to the filtering LLM with a system prompt requesting ranked relevant files. The filter model is the registry's shared cheap/fast model (`ModelRegistry::get_cheap_model` via `LlmSelector`): `claude-haiku-4-5`, then `gpt-5.4-mini`, then the default service — so it resolves correctly whether the deployment has Anthropic models, OpenAI/GPT models, or both, with no keyword_search-local model list to drift.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-KWS-001:** Conceptual Search | ✅ Complete | Two-stage ripgrep + LLM filtering |
| **REQ-KWS-002:** Search Scope | ✅ Complete | Git root detection, case-insensitive |
| **REQ-KWS-003:** Result Filtering | ✅ Complete | LLM filters with relevance prompt |
| **REQ-KWS-004:** Tool Schema | ✅ Complete | query + search_terms array |
| **REQ-KWS-005:** LLM Selection | ✅ Complete | Prefers fast models, falls back |

**Progress:** 5 of 5 complete
