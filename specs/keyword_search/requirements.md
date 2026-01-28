# Keyword Search Tool

## User Story

As an LLM agent, I need to search unfamiliar codebases by concept when I don't have precise information like filenames, symbols, or error messages.

## Requirements

### REQ-KWS-001: Conceptual Search

WHEN agent requests keyword search with query and search terms
THE SYSTEM SHALL search the repository using ripgrep with provided terms
AND filter results for relevance using an LLM
AND return ranked list of relevant files with explanations

WHEN search terms yield too many results (>64KB per term)
THE SYSTEM SHALL skip overly broad terms
AND continue with remaining terms

WHEN all terms yield too many results
THE SYSTEM SHALL return error indicating terms are too broad

**Rationale:** LLMs navigating unfamiliar codebases need conceptual search. Raw ripgrep output is often too noisy; LLM filtering provides relevant results.

---

### REQ-KWS-002: Search Scope

WHEN keyword search executes
THE SYSTEM SHALL search from git repository root if in a git repo
AND fall back to conversation working directory otherwise

WHEN searching
THE SYSTEM SHALL use case-insensitive matching
AND include 10 lines of context around matches
AND include filenames and line numbers

**Rationale:** Repository root provides complete codebase coverage. Context helps the filtering LLM understand relevance.

---

### REQ-KWS-003: Result Filtering

WHEN ripgrep returns results
THE SYSTEM SHALL send results to a fast, cheap LLM for relevance filtering
AND include the original query for context
AND request ranked list of genuinely relevant files

WHEN filtering LLM responds
THE SYSTEM SHALL return the filtered results to the agent

**Rationale:** Two-stage search (grep then filter) balances speed with relevance. Fast models keep latency acceptable.

---

### REQ-KWS-004: Tool Schema

WHEN LLM requests keyword_search tool
THE SYSTEM SHALL provide schema with:
- `query` (required string): Detailed statement of what to find
- `search_terms` (required array of strings): Terms in descending order of importance

WHEN providing tool description
THE SYSTEM SHALL advise:
- Use many specific terms over few general ones
- Order terms by importance (most important first)
- Do NOT use this tool when precise information is available

**Rationale:** Clear guidance helps agents use the tool effectively. Term ordering enables graceful degradation when results are too large.

---

### REQ-KWS-005: LLM Selection

WHEN selecting LLM for result filtering
THE SYSTEM SHALL prefer fast, cheap models in order:
1. Fireworks Qwen coder
2. GPT-5 mini variants
3. Claude Sonnet
4. Any available model

WHEN no LLM is available
THE SYSTEM SHALL return error

**Rationale:** Keyword search is a high-frequency tool; using expensive models would be cost-prohibitive. Fast models provide adequate filtering quality.
