# Add Cmd+P code content search results

## Goal

Make Cmd+P useful when the user knows a string exists inside the active conversation's repo, not just when the string appears in a file path.

Example target behavior:

```text
Cmd+P query: metricSourceToOriginProduct

Files
  main.go — pkg/collector/corechecks/gpu/spec/allowlist

Code
  metricSourceToOriginProduct — pkg/collector/corechecks/gpu/spec/allowlist/main.go:42
  originProduct := metricSourceToOriginProduct[source]
```

Selecting a `Code` result opens the existing metaviewer/prose panel to the matching file and highlights/jumps to the matching line.

## Scope

Implement an MVP content-search source for the command palette:

1. Add a conversation-scoped backend API for content search under the conversation file root.
   - Respect `.gitignore`, `.ignore`, global gitignore, and explicit `.git/` exclusion.
   - Search text files only.
   - Use literal substring matching, not fuzzy content matching.
   - Use smart-case semantics:
     - all-lowercase query matches case-insensitively;
     - mixed-case query matches case-sensitively.
   - Return bounded, typed results with at least: relative path, line number, line text, and match span where practical.
   - Cap result count and avoid unbounded work on large repositories.

2. Add a `Code` command-palette source alongside the existing `Files` source.
   - Empty query returns no code results.
   - Query results are debounced/abortable like existing palette sources.
   - Code results are visually distinct from file-path results via category/title/subtitle/snippet.

3. Open code search hits in the metaviewer/prose file panel.
   - Selecting a code hit opens the matched file using the active conversation file root.
   - The matched line is highlighted and scrolled into view.
   - Preserve the existing file-path search behavior.

4. Tests / validation.
   - Backend tests for smart-case matching, gitignore/root scoping, result bounds, and line metadata.
   - Frontend tests for mapping code search results into palette items and selecting a hit.
   - Run the normal Phoenix checks through `./dev.py check` before committing.

## Non-goals

- Tantivy or other persistent indexing.
- LSP/workspace-symbol search.
- Fuzzy matching within code contents.
- A broad redesign of Cmd+P modes/prefixes.

Those can be layered behind the same UI/API concepts later if latency or symbol navigation becomes the main problem.
