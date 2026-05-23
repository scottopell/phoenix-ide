# Follow-up React UI performance opportunities

Suggested p1 follow-ups after the React UI performance pass that produced two approved large-conversation load wins and several recorded rejections.

## Context

The `conversation-load` scenario is now runnable via the deterministic `fixture-turn-one` seed fixture. The two approved wins so far were both in large finalized agent-message rendering:

- on-demand Prism language registration
- conditional `remark-gfm` parsing only when GFM syntax is present

Both cleared the shared perf gate on `conversation-load` with `runs=20`, `warmup=3`, `throttle_rate=4`.

## Suggested opportunities

### Re-profile conversation-load after the two wins

The next target should start with a fresh CPU profile and a fresh baseline on current `main`/branch state. Earlier profiles pointed at markdown and syntax-highlighter work; after the two wins, the hot path may have shifted.

### Continue investigating finalized markdown/code rendering

The remaining load cost is likely still in large agent message rendering. Areas worth checking:

- `ReactMarkdown` parsing/rendering cost for large plain text blocks
- syntax-highlighter element creation for repeated code fences
- whether large historical code blocks need full highlighting during initial load
- whether markdown rendering work can be staged without hurting perceived correctness

### Re-test message-row `content-visibility` only as a new combined hypothesis

Standalone `.message { content-visibility: auto }` moved metrics in the right direction but missed thresholds. Now that markdown work is lower, browser layout/paint may be a larger share. If revisited, treat it as a new post-markdown-load hypothesis, not the already-recorded standalone technique.

### Add a dedicated large-conversation scroll scenario

Current `conversation-load` measures list-to-conversation navigation and initial render. It does not measure interaction after load. A separate scenario could cover:

- scrolling through a large conversation
- jumping to newest / restored scroll positions
- expanding/collapsing tool blocks
- right-click/context menu behavior in a large message list

### Improve fixture coverage for message shapes

`fixture-turn-one` currently gives the load path a deterministic large conversation. Additional fixtures could make future hunts more representative:

- many tool use/result pairs
- many skill invocations
- tables/task lists/footnotes-heavy markdown
- long code blocks in multiple languages
- image/tool-result-heavy messages

### Revisit bundle/load split after runtime wins

The build output still has large chunks for syntax highlighting, connection/hooks, and terminal. Runtime load wins came from doing less work after those chunks load. A future pass could check whether initial route chunks still include avoidable work for list-only and conversation-only paths.

## Guardrails

- Keep using raw per-run samples and the shared gate.
- Record rejected outcomes; they are useful.
- Do not merge “obviously cheaper” changes without threshold-clearing measurements.
- Keep scenarios deterministic before making more changes.
