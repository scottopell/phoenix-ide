# Disclosure interpretation integration

`../remarkDetails.ts` is the public entry point:

```ts
const interpretation = interpretMarkdown(source, basicOptions);
// Or explicitly gfmOptions where the renderer uses remark-gfm.
const { tree, ranges, definitions, stats } = interpretation;
```

The result does not retain another copy of source. All offsets are UTF-16 source
positions, including the original BOM and CRLF coordinates. `ranges` contains
accepted top-level disclosure envelopes. Definitions are source-ordered,
first-wins native decoded nodes, sharing identity with nodes in `tree`.

- `markdownDetailsRanges(interpretation)` returns those same ranges without parsing.
  Its string overload is a convenience for callers without an interpretation.
- `markdownReferenceContext(interpretation)` returns that interpretation. **It no
  longer returns a content-to-string function.** Update old callers rather than
  appending definition text to slices. The string overload interprets once.
- `remarkDetails` installs `processor.parser`, after remark-parse. At parse
  invocation it reads the processor's real settings, micromark extensions and
  from-Markdown extensions; it does not enable GFM itself or reparse a transformer
  input. It is intended for the conversation renderer only.
- `[remarkDetails, { tree: selectedRoot }]` lets ReactMarkdown/unified consume an
  already selected canonical root without parsing its string child again. The
  selector must preserve complete nodes and their original source positions.
  Selection/chunking must not split accepted ranges or reparse isolated bodies.
  The supplied tree must have been interpreted with the renderer's configuration.
- Find should walk this same tree iteratively. Disclosure body and summary are
  children of the canonical disclosure node, not a second hidden source string.

## Typed structural nodes

The prototype's `blockquote`-tagged summary held phrasing children, which is not
valid mdast typing. The port uses registered `disclosureDetails` and
`disclosureSummary` parent types instead. Their `data.hName` values are `details`
and `summary`; native mdast-to-hast handles these without a bespoke renderer.
Summary children are phrasing; details begin with exactly one summary. The only
retained opening-tag property is boolean `open`. Generic node consumers must not
assume that a details node's mdast `type` is `blockquote`.

Keep raw HTML reparsing disabled and retain ReactMarkdown's URL policy. Ordinary
HTML remains native HTML nodes, and rejected disclosure tags are native literal
text. This module is not a raw-HTML sanitizer or a replacement URL policy.

## References and chunking

Real document/body definitions are collected before summaries compile. Lexical
recognition's syntax-first membership is separate from those real definitions.
When disclosure tags are recognized, native references become concrete links or
images against the global first-wins definition map. Documents with no recognized
tags preserve the ordinary native reference nodes. A selector for the latter must
resolve their referenced definitions in AST form or carry only needed definition
nodes into the selected root; it must not append definition strings or collect
all definitions separately for every chunk.

## Admission handoff

Runtime imports use the pinned static shipping seams in `ui/patches`, never
private dependency paths or generated source. Additional direct type dependencies
needed by this port are `micromark-util-types@2.0.2` and `@types/mdast@4.0.4`.
The supplied `ControlledContext`/`BorrowedContext` declarations match the port's
calls. Upstream `ConstructRecord` allows singular entries even after native
normalization; `constructList` handles that declaration without unsafe casts.

No install, test, build, or typecheck was run during this port: isolated host
admission is owned by the parent. Run the v6 semantic/native-parity gates against
this static port, then strict TypeScript, lint, development/production import
conditions and the production bundle. There is no spike input-size cap.
