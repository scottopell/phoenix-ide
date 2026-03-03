---
created: 2026-03-01
number: 592
priority: p2
status: ready
slug: streaming-markdown-rendering
title: "Streaming markdown rendering: block-aware progressive display"
---

# Streaming Markdown Rendering

## Problem

`StreamingMessage` renders raw `buffer.text` as plain text. When the response
completes, `AgentMessage` renders the same content through ReactMarkdown + Prism
syntax highlighting. The swap causes a jarring layout jump — the entire message
reflows at once as block-level elements (code blocks, tables, headings) take their
final dimensions.

## Design: Block-Aware Streaming Renderer

The approach: parse the streaming buffer into blocks, render each block
appropriately for its completion state. **Do not hide incomplete blocks — display
them in layout-stable containers so the final swap is invisible except for styling.**

### The Core Function

One pure function, no state between calls. Called on every render with the full
accumulated buffer:

```typescript
type StreamingBlock =
  | { type: 'markdown'; content: string }
  | { type: 'code'; lang: string; content: string; complete: boolean };

function parseStreamingBlocks(buffer: string): StreamingBlock[]
```

The function walks lines tracking only three things:
1. Am I inside a fenced code block? (which character, what length)
2. Am I inside an HTML block? (which type)
3. Is the last line terminated?

Everything outside an open code fence or HTML block is emitted as a `markdown`
block (rendered through ReactMarkdown). Fenced code regions are emitted as `code`
blocks with a `complete` flag.

### Rendering Rules

| Block type | Complete? | Render as |
|-----------|-----------|-----------|
| `markdown` | n/a | ReactMarkdown (same component as `AgentMessage`) |
| `code` | `true` | SyntaxHighlighter with Prism (same as `AgentMessage`) |
| `code` | `false` | `<pre><code className="streaming-code">` — plain monospace, no highlighting |

**The key insight:** an open code block renders inside `<pre><code>` with CSS that
matches the SyntaxHighlighter's dimensions exactly — same font family, padding,
border-radius, background color. When the closing fence arrives and `complete` flips
to `true`, the swap to SyntaxHighlighter is invisible except colors appearing on
already-visible text. No layout shift.

### CSS Requirements

```css
.streaming-code {
  /* Must match oneDark/SyntaxHighlighter dimensions exactly */
  font-family: 'SF Mono', Monaco, Consolas, monospace;
  font-size: /* same as syntax highlighter */;
  padding: /* same as syntax highlighter */;
  background: /* same as syntax highlighter */;
  border-radius: /* same as syntax highlighter */;
  white-space: pre;
  overflow-x: auto;
}
```

### Fence Detection Algorithm

```
walk lines forward:
  if inside_fence:
    if line matches closing fence (same char, same or greater length):
      close fence, emit code block with complete=true
    else:
      accumulate line into current code block
  else:
    if line matches opening fence (3+ backticks or tildes):
      flush accumulated markdown block
      open fence, record char + length + lang
    else:
      accumulate line into current markdown block

at end:
  if inside_fence:
    emit code block with complete=false
  if accumulated markdown:
    emit markdown block
```

HTML block detection (types 1-5 per CommonMark) can be added as a second tracked
state using the same pattern. Types 6-7 (close on blank line) are safe to render
progressively.

### What to Punt On (v1)

- **Tables:** Render progressively as markdown. Partial rows will flicker briefly
  but tables are ~5% of output. If this proves annoying, add table row buffering
  in v2.
- **Indented code blocks:** Treat as prose (render through markdown). They're rare
  in LLM output; fenced blocks dominate.
- **Nested blockquotes with lazy continuation:** Accept imperfect rendering.
- **Definition link references:** Unresolved refs flicker briefly. Acceptable.

### Frame-Aligned Rendering

Use `requestAnimationFrame` gating (NOT debouncing) to coalesce tokens that arrive
within a single 16ms frame:

```typescript
const pendingBuffer = useRef<string>("");
const rafHandle = useRef<number | null>(null);

function onToken(chunk: string) {
  pendingBuffer.current += chunk;
  if (rafHandle.current === null) {
    rafHandle.current = requestAnimationFrame(() => {
      setDisplayBuffer(pendingBuffer.current);
      rafHandle.current = null;
    });
  }
}
```

This avoids 15 React renders/second without introducing the artificial latency
that debouncing causes. Cancel the pending rAF in cleanup.

## Testable Properties

The `parseStreamingBlocks` function is pure and can be property-tested:

**P1: Concatenating block contents reproduces the input**
```
blocks.map(b => b.content).join('') === buffer  // (modulo fence lines)
```

**P3: Monotonicity — block count never decreases as buffer grows**
```
parseStreamingBlocks(buffer + suffix).length >= parseStreamingBlocks(buffer).length
```

**P5: No open code fence in markdown blocks**
```
for each block where type === 'markdown':
  count of unmatched fence openers === 0
```

**P9: Block boundaries are at line boundaries**
```
every block.content either ends with '\n' or is the last block
```

## Acceptance Criteria

- [ ] `parseStreamingBlocks` exists as a pure function with unit tests
- [ ] Property tests verify P1, P3, P5, P9
- [ ] Prose renders through ReactMarkdown during streaming (bold, links, etc.)
- [ ] Open code blocks render as plain monospace in layout-stable container
- [ ] Closed code blocks render with syntax highlighting
- [ ] No layout jump when streaming completes and `AgentMessage` replaces `StreamingMessage`
- [ ] rAF gating on token updates (no debounce)
- [ ] No regression in non-streaming (complete message) rendering
- [ ] Latency to first visible token remains under ~200ms
- [ ] `./dev.py check` passes

## Files Likely Involved

- `ui/src/components/StreamingMessage.tsx` — replace raw text with block renderer
- `ui/src/utils/parseStreamingBlocks.ts` — NEW: the pure parsing function
- `ui/src/utils/parseStreamingBlocks.test.ts` — NEW: unit + property tests
- `ui/src/index.css` — `.streaming-code` matching SyntaxHighlighter dimensions
- `ui/src/conversation/atom.ts` — rAF gating for token updates (or in useConnection)

## Research Sources

This design was informed by four independent expert analyses:

- **Property testing:** 9 invariants defined, P3 (monotonicity) identified as the
  critical property for catching rendering regressions.
- **Markdown parsing:** Only three pieces of state needed (fence open, HTML block
  open, line terminated). No existing JS library provides the right API shape —
  roll our own ~50-80 line function.
- **Performance:** Pure function viable for <50KB buffers. Markdown renderer
  (ReactMarkdown) is the bottleneck, not the scan. rAF > debounce.
- **UX:** Layout stability is the #1 perceived quality signal. Claude.ai uses the
  same pattern: open `<pre>` immediately on fence, stream plain text, highlight on
  close. The swap is invisible except colors appearing.
