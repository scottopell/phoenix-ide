# Keyword Search Tool

## User Story

As an LLM agent, I need to search unfamiliar codebases by concept when I don't have precise information like filenames, symbols, or error messages.

## Requirements

### REQ-KWS-001: Conceptual Search

WHEN agent requests keyword search with query and search terms
THE SYSTEM SHALL search the repository using ripgrep with provided terms
AND filter results for relevance using an LLM
AND return ranked list of relevant files with explanations

WHEN a search term matches more than a breadth threshold of locations
THE SYSTEM SHALL skip that term
AND continue with remaining terms
AND determine breadth without buffering the term's full contextual output

WHEN all terms exceed the breadth threshold
THE SYSTEM SHALL return error indicating terms are too broad

WHEN combined contextual output exceeds a fixed size ceiling
THE SYSTEM SHALL stop the scan at the ceiling
AND signal that results were truncated

**Rationale:** LLMs navigating unfamiliar codebases need conceptual search. Raw ripgrep output is often too noisy; LLM filtering provides relevant results. Breadth and output cost must be bounded independently of the size of the search tree, so an intentionally broad working directory stays searchable rather than stalling — measuring a term's breadth by first producing its full output would pay the exact cost the skip is meant to avoid.

---

### REQ-KWS-002: Search Scope

WHEN keyword search executes
THE SYSTEM SHALL search from git repository root if in a git repo
AND fall back to conversation working directory otherwise

WHEN the resolved search root is the filesystem root
THE SYSTEM SHALL refuse the search

WHEN searching
THE SYSTEM SHALL use case-insensitive matching
AND include 10 lines of context around matches
AND include filenames and line numbers

**Rationale:** Repository root provides complete codebase coverage. Context helps the filtering LLM understand relevance. A working directory spanning many repositories is a supported, intentional scope and is not refused for breadth alone; the filesystem root is refused because it would scan every mounted volume, and bounded cost is enforced by the breadth threshold and output ceiling (REQ-KWS-001) rather than by narrowing the user's chosen scope.

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
THE SYSTEM SHALL use the application's shared cheap/fast model selection
AND that selection SHALL span the supported providers and fall back to the default model

WHEN no LLM is available
THE SYSTEM SHALL return error

**Rationale:** Keyword search is a high-frequency tool; using expensive models would be cost-prohibitive, and fast models provide adequate filtering quality. Filtering reuses the one shared cheap-model selector rather than a tool-local list, so it resolves correctly under every deployment shape — Anthropic models only, OpenAI/GPT models only, or both — and cannot drift from the rest of the app. The concrete model roster is an implementation detail that changes as models are added and retired, so it lives in the code (the shared selector) and `executive.md`, not pinned in a normative requirement where it would rot into spec-versus-code conflicts.
