# Fix streaming markdown parser for fenced markdown payloads with nested fences

## Problem

A stress-test message wrapped in an outer ```` ```markdown ```` fence contains many inner triple-backtick fences (`text`, `mermaid`, destination paths, and examples). The frontend streaming markdown parser currently tokenizes fenced code blocks with a single top-level CommonMark-style fence state in `ui/src/utils/parseStreamingBlocks.ts` and renders complete fences outside `ReactMarkdown` via `StreamingMessage.tsx`.

That works for ordinary code fences, but it can choke on this common LLM pattern:

- The model emits an outer `markdown` code fence to quote a full markdown document.
- The quoted document itself contains same-length inner code fences.
- The streaming parser treats the first inner ` ``` ` line as closing the outer fence, then misclassifies the rest of the quoted markdown as live conversation markdown/code.
- During streaming this can cause severe layout churn and broken rendering because block identity flips as more nested fences arrive.

## Goal

Make streamed markdown/code-fence rendering robust for fenced markdown documents that contain nested fenced blocks, without regressing the existing progressive rendering behavior for ordinary prose and code blocks.

## Investigation plan

1. Add a focused regression fixture using the concrete minimized stress input below.
   - Exercise both complete-buffer parsing and token-by-token/substring streaming.

```text
```markdown
You are starting fresh in the `datadog-agent` repository on a new branch.

## Context

Recommended destination:

```text
tools/check-skip-observability-demo/index.html
```

## Current Agent behavior to understand

### Scheduler

```text
pkg/collector/scheduler/job.go
```

The worker logs something similar to:

```text
Check is already running, skipping execution...
```

## Desired design direction

```text
time →
scheduled ticks:        |   |   |   |   |
actual run spans:       [=======]   [=======]
skipped attempts:           x           x
current gauge samples:          ●           ●
```

## Deliverable

Create or replace:

```text
tools/check-skip-observability-demo/index.html
```
```
```

2. Reproduce the current incorrect block segmentation in `parseStreamingBlocks` and/or a component-level render test for `StreamingMessageView`.
3. Decide the smallest correct parser behavior:
   - Prefer preserving the whole outer markdown fence as a single code block when it is clearly quoting a markdown document, or
   - Otherwise choose a structural escape strategy that keeps block boundaries stable while streaming and prevents inner fences from escaping the quoted document.
4. Keep existing guarantees covered by tests:
   - ordinary fenced code blocks still syntax-highlight after closing,
   - incomplete fences still render as plain monospace,
   - mermaid fences still render through `MermaidDiagram`,
   - tables/links in normal markdown still render via `ReactMarkdown`,
   - no per-token throw or pathological block churn on the stress fixture.

## Acceptance criteria

- The provided nested-fence stress example renders intelligibly in a streaming message.
- Inner fences inside an outer `markdown` quoted document no longer break the outer block prematurely.
- Streaming partial prefixes of the stress fixture do not throw and do not produce severe block-boundary churn after completed blocks.
- Existing parser and message rendering tests pass.
- Add or update tests near `ui/src/utils/parseStreamingBlocks.test.ts` and, if needed, `ui/src/components/MessageComponents.test.tsx`.
- Run the relevant UI tests, then `./dev.py check` before committing.
