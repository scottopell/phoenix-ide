# Keyword Search Tool - Design Document

## Overview

Keyword search is a two-stage tool: first ripgrep searches for terms, then an LLM filters results for relevance. This provides conceptual code search when the agent doesn't have precise information.

## Tool Interface (REQ-KWS-004)

### Schema

```json
{
  "type": "object",
  "required": ["query", "search_terms"],
  "properties": {
    "query": {
      "type": "string",
      "description": "A detailed statement of what you're trying to find or learn."
    },
    "search_terms": {
      "type": "array",
      "items": { "type": "string" },
      "description": "List of search terms in descending order of importance."
    }
  }
}
```

### Description

```
keyword_search locates files with a search-and-filter approach.
Use when navigating unfamiliar codebases with only conceptual understanding or vague user questions.

Effective use:
- Provide a detailed query for accurate relevance ranking
- Prefer MANY SPECIFIC terms over FEW GENERAL ones (high precision beats high recall)
- Order search terms by importance (most important first)
- Supports regex search terms for flexible matching

IMPORTANT: Do NOT use this tool if you have precise information like log lines, error messages, stack traces, filenames, or symbols. Use direct approaches (rg, cat, etc.) instead.
```

## Execution Flow (REQ-KWS-001, REQ-KWS-002, REQ-KWS-003)

```rust
struct KeywordSearchInput {
    query: String,
    search_terms: Vec<String>,
}

impl KeywordSearchTool {
    pub async fn run(&self, input: KeywordSearchInput) -> ToolResult {
        // 1. Determine search root
        let search_dir = self.find_repo_root()
            .unwrap_or_else(|| self.working_dir.clone());
        
        // 2. Filter out overly broad terms
        let mut usable_terms = Vec::new();
        for term in &input.search_terms {
            let result = self.ripgrep(&search_dir, &[term]).await?;
            if result.len() <= 64 * 1024 {
                usable_terms.push(term.clone());
            }
        }
        
        if usable_terms.is_empty() {
            return ToolResult::Error(
                "each of those search terms yielded too many results".to_string()
            );
        }
        
        // 3. Search with usable terms, peeling off until results fit
        let mut results = String::new();
        while !usable_terms.is_empty() {
            results = self.ripgrep(&search_dir, &usable_terms).await?;
            if results.len() < 128 * 1024 {
                break;
            }
            usable_terms.pop();
        }
        
        // 4. Filter results with LLM
        let filtered = self.filter_with_llm(&input.query, &search_dir, &results).await?;
        
        ToolResult::Success(filtered)
    }
}
```

## Ripgrep Invocation (REQ-KWS-002)

```rust
async fn ripgrep(&self, dir: &Path, terms: &[String]) -> Result<String, Error> {
    let mut args = vec![
        "-C", "10",           // 10 lines context
        "-i",                  // Case insensitive
        "--line-number",
        "--with-filename",
    ];
    
    for term in terms {
        args.push("-e");
        args.push(term);
    }
    
    let output = Command::new("rg")
        .args(&args)
        .current_dir(dir)
        .output()
        .await?;
    
    // Exit code 1 = no matches (not an error)
    if output.status.code() == Some(1) {
        return Ok("no matches found".to_string());
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
```

## LLM Filtering (REQ-KWS-003, REQ-KWS-005)

### System Prompt

```
You are a code search relevance evaluator. Your task is to analyze ripgrep results and determine which files are most relevant to the user's query.

INPUT FORMAT:
- You will receive ripgrep output containing file matches for keywords with 10 lines of context
- At the end will be the original search query

ANALYSIS INSTRUCTIONS:
1. Examine each file match and its surrounding context
2. Evaluate relevance to the query based on:
   - Direct relevance to concepts in the query
   - Implementation of functionality described in the query
   - Evidence of patterns or systems related to the query
3. Exercise strict judgment - only return files that are genuinely relevant

OUTPUT FORMAT:
Respond with a plain text list of the most relevant files in decreasing order of relevance:

/path/to/most/relevant/file: Concise relevance explanation
/path/to/second/file: Concise relevance explanation
...

IMPORTANT:
- Only include files with meaningful relevance to the query
- Keep it short, don't blather
- Do NOT list all files that had keyword matches
- Focus on quality over quantity
- If no files are truly relevant, return "No relevant files found"
- Use absolute file paths
```

### LLM Selection

```rust
const PREFERRED_MODELS: &[&str] = &[
    "qwen3-coder-fireworks",
    "gpt-5-thinking-mini",
    "gpt5-mini",
    "claude-sonnet-4.5",
];

async fn select_filter_llm(&self) -> Result<Arc<dyn LlmService>, Error> {
    for model in PREFERRED_MODELS {
        if let Some(svc) = self.llm_registry.get(model) {
            return Ok(svc);
        }
    }
    
    // Fall back to any available model
    self.llm_registry.available_models()
        .first()
        .and_then(|m| self.llm_registry.get(m))
        .ok_or_else(|| Error::NoLlmAvailable)
}
```

## Testing Strategy

### Unit Tests
- Term filtering (skip terms with >64KB results)
- Result truncation (peel terms until <128KB)
- Ripgrep argument construction

### Integration Tests
- Full search flow with mock LLM
- Git root detection
- Fallback to working directory

## File Organization

```
src/tools/
├── keyword_search/
│   ├── mod.rs
│   ├── ripgrep.rs       # Ripgrep invocation
│   ├── filter.rs        # LLM filtering
│   └── prompts.rs       # System prompt
```
