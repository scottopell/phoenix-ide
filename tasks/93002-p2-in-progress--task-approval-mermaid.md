# Make Phoenix Markdown render Mermaid consistently

## Problem

Phoenix Markdown rendering is inconsistent across the application. Some Markdown surfaces already render fenced `mermaid` code blocks as diagrams with `MermaidDiagram`, but the task approval reader renders the same Markdown as a syntax-highlighted code block. A task body containing a Mermaid diagram should look like a diagram everywhere Phoenix presents rendered Markdown, including task approval.

Observed code:

- `ui/src/components/TaskApprovalReader.tsx` has a custom `ReactMarkdown` `code` component that syntax-highlights every fenced language.
- `ui/src/components/StreamingMessage.tsx` and `ui/src/components/viewer/MarkdownViewerBody.tsx` already special-case `mermaid` and render `<MermaidDiagram code={...} />`.
- `ui/src/components/ForkProposalReview.tsx` reuses task-approval visual chrome and has the same raw-code-only Markdown renderer, so it likely has the same consistency gap for task/fork proposal bodies.

## Scope

Make Phoenix Markdown handling consistent for Mermaid diagrams on task approval and adjacent proposal-review surfaces, preserving existing annotation and feedback behavior.

## Plan

1. Update `TaskApprovalReader` Markdown code rendering:
   - import `MermaidDiagram`.
   - detect fenced code language with a regex that accepts full language tokens, not only `\w+`.
   - when language lowercases to `mermaid`, render `<MermaidDiagram code={String(children)} />` instead of `SyntaxHighlighter`.
   - keep inline code and non-Mermaid fences unchanged.

2. Add regression test coverage:
   - render a `TaskApprovalReader` plan containing a fenced `mermaid` block.
   - mock `MermaidDiagram` or assert its `data-testid="mermaid-diagram"` appears.
   - assert the task approval Markdown surface uses the same Mermaid path as other Phoenix Markdown surfaces.

3. Consider shared helper cleanup if small:
   - `TaskApprovalReader`, `MarkdownViewerBody`, and `ForkProposalReview` duplicate fenced-code rendering logic.
   - If low-risk, extract a small shared Markdown code component/helper that handles Mermaid consistently.
   - If extraction grows scope, consult user via AskUserQuestion tool.

4. Check adjacent drift:
   - If `ForkProposalReview` is meant to render task/proposal Markdown with the same expectations, apply the same Mermaid special-case there too and add/adjust a focused test.

## Validation

Run targeted UI tests first:

```bash
./dev.py check --lanes ui
./dev.py qa meta-viewer
```

If lane names differ or the gated check chooses more work, follow repo tooling output. At minimum, run the relevant Vitest target for `TaskApprovalReader` after the change.
