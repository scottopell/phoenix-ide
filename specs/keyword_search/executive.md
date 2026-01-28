# Keyword Search Tool - Executive Summary

## Requirements Summary

Keyword search enables conceptual code search when agents lack precise information. It uses a two-stage approach: ripgrep searches for provided terms, then an LLM filters results for relevance. Search runs from git repository root (or working directory fallback) with case-insensitive matching and 10 lines of context. Overly broad terms (>64KB results) are automatically skipped. The filtering LLM is selected from fast, cheap models to keep latency and cost acceptable.

## Technical Summary

Tool accepts query string and ordered search terms array. Ripgrep runs with `-C 10 -i --line-number --with-filename -e <term>` for each term. Terms yielding >64KB are skipped; combined results are trimmed by removing lowest-priority terms until <128KB. Results plus query are sent to filtering LLM with system prompt requesting ranked relevant files. LLM selection prefers Fireworks Qwen, GPT-5 mini, then Claude Sonnet.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-KWS-001:** Conceptual Search | ✅ Complete | Two-stage ripgrep + LLM filtering |
| **REQ-KWS-002:** Search Scope | ✅ Complete | Git root detection, case-insensitive |
| **REQ-KWS-003:** Result Filtering | ✅ Complete | LLM filters with relevance prompt |
| **REQ-KWS-004:** Tool Schema | ✅ Complete | query + search_terms array |
| **REQ-KWS-005:** LLM Selection | ✅ Complete | Prefers fast models, falls back |

**Progress:** 5 of 5 complete
