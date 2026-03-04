---
created: 2026-03-01
number: 592
priority: p2
status: ready
slug: streaming-markdown-rendering-qaplan
title: "QA Plan: Streaming markdown rendering"
---

# QA Plan: Task 592 — Streaming Markdown Rendering

This QA plan is executed by a separate agent AFTER the implementation agent
completes. The QA agent should not read the implementation code first — test from
the spec and user-visible behavior.

## Setup

1. Ensure Phoenix is running: `./dev.py restart`
2. Note the Vite port from `./dev.py status`
3. All tests use `phoenix-client.py` or direct browser tools

## Test 1: Prose renders as markdown during streaming

Send a message that produces a long prose response with inline formatting:

```
./phoenix-client.py -m claude-haiku-4-5 "Write a 3-paragraph explanation of Rust's ownership model. Use **bold** for key terms, `backtick` for code concepts, and include a [link](https://doc.rust-lang.org) reference."
```

**Verify:** In the browser (via Vite URL), watch the streaming response. Bold text,
inline code, and the link should render progressively during streaming — not as raw
`**bold**` asterisks. Check by navigating to the conversation in the browser and
observing the stream live, or by checking the final rendered output includes proper
formatting.

**Pass criteria:** Inline markdown renders during streaming. No raw asterisks or
backticks visible in the final output.

## Test 2: Code block renders in monospace container during streaming

Send a message that produces a code block:

```
./phoenix-client.py -m claude-haiku-4-5 "Write a Rust function that implements binary search. Put it in a fenced code block with rust language tag."
```

**Verify:**
- During streaming: code should appear inside a monospace container (dark
  background, monospace font) — NOT as raw text inline with prose
- After streaming completes: syntax highlighting should appear (colored keywords)
- No layout jump when highlighting activates — the container size should stay the
  same

**Pass criteria:** Code block has a container during streaming. No visible layout
shift on completion.

## Test 3: Mixed prose and code — no layout jump on completion

Send a message designed to produce prose then code then prose:

```
./phoenix-client.py -m claude-haiku-4-5 "Explain what a HashMap is in one sentence, then show a 10-line Rust example in a code block, then explain the output in one sentence."
```

**Verify:** Watch the streaming in the browser. The prose-to-code-to-prose
transitions should be smooth. When the final message replaces the streaming view,
there should be no visible jump or reflow.

**Pass criteria:** Smooth transitions between block types. No jarring reflow on
completion.

## Test 4: Multiple code blocks in one response

```
./phoenix-client.py -m claude-haiku-4-5 "Show me three small code examples: one in Python, one in JavaScript, one in Rust. Each should be 3-5 lines in a fenced code block with the language tag."
```

**Verify:** Each code block gets its own container during streaming. Syntax
highlighting applies independently as each block's fence closes.

**Pass criteria:** Three separate code containers visible. Each highlights
independently.

## Test 5: Non-streaming messages unaffected

Navigate to an existing completed conversation in the browser. Verify that
previously completed messages still render with full markdown + syntax highlighting.
No regression.

**Pass criteria:** Old messages render identically to before.

## Test 6: parseStreamingBlocks unit tests exist and pass

```
cd ui && npx vitest run parseStreamingBlocks 2>&1
```

**Pass criteria:** Tests exist and pass. Should include property-style tests for
monotonicity (P3) and fence parity (P5).

## Test 7: Performance — no visible lag

Send a message that produces a long response (500+ tokens):

```
./phoenix-client.py -m claude-haiku-4-5 "Write a detailed tutorial on building a REST API in Rust with axum. Include at least 3 code examples."
```

**Verify:** Tokens appear smoothly without stuttering or visible batching delays.
The stream should feel like continuous typing.

**Pass criteria:** No perceptible lag between token arrival and display update.

## Report Format

Write a brief report:
- Which tests passed / failed
- Screenshots or observations of any layout jumps
- Any edge cases discovered
- Overall assessment: does streaming feel professional?

DO NOT modify any code. This is read-only QA.
