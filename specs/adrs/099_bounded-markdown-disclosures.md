# ADR-099: Recognize bounded Markdown disclosures within native parsing contexts

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-CONV-025; REQ-CONV-002 and REQ-CONV-019 — conversation Markdown rendering and streaming

## Context

Phoenix needs author-provided disclosures without enabling arbitrary HTML or changing ordinary Markdown semantics. The user selected a bounded recognizer replacement inside the existing micromark/mdast stack. Rendering, streamed chunk selection, and conversation find must agree on boundaries, definitions, and original source positions. File and plan viewers have different rendering scope and must not inherit conversation-only find transformations.

The historical masking approach exposed a circular dependency: disabling HTML flow let a script's unmatched backticks hide a later wrapper; reparsing then exposed that wrapper without rediscovering its same-line opaque script body. An apparent closing tag inside the script was accepted and real body text escaped the disclosure. Additional masking rounds repaired examples but did not establish completeness. Subsequent prototypes also exposed native lazy-container continuation, GFM table/footnote context, multiline attributes, and phrasing-versus-flow summary distinctions. These are context-ownership problems, not missing tag patterns.

Reference labels introduce a separate precedence choice. If recognition depends on eventual definition resolution, a definition's own visibility can depend on the wrapper boundary that recognition is trying to decide. Global approved syntax-first shielding for native-complete reference-label constructs, whether or not a real definition exists. Undefined references remain visible literals; malformed reference syntax does not gain blanket shielding. This is Phoenix extension precedence, not a claim that CommonMark mandates it.

## Options considered

1. **More masks, reparses, or fixed-point discovery** — small local changes, but input-dependent rounds or a hard cap trade bounded work against correct interpretation; overlapping suffix parsing amplifies work with candidate count.
2. **A new Markdown framework or vendored parser/compiler** — broad control, but a second grammar and compiler ownership burden, with unnecessary basic/GFM and source-position parity risk.
3. **Bounded native-context recognition with two narrow pinned package patches** — preserves the existing grammar and compiler while exposing the finite-input controls needed to honor native context ownership. It carries explicit patch maintenance and qualification costs.

## Decision

Choose option 3. Use static exports from pinned `micromark@4.0.2` and `mdast-util-from-markdown@2.0.3`, with narrow pnpm patches in both development and production variants. The micromark seam exposes native preprocessing, parsing, postprocessing, document initialization, and controlled tokenization; the mdast seam exports the existing native event compiler rather than copying or replacing it. Other directly imported native helpers are declared dependencies. Runtime source rewriting, private-file loading, data-URL loaders, and eval are excluded from shipping code.

The recognizer operates on a finite preprocessed tape and original indexed points. Borrowed sessions cannot append to that tape; exclusive-offset EOF is reversible with speculative state. Terminal yield is not resumable pause. Native document admission identifies root flow, while reversible container depth includes quotes, lists, and GFM footnote definitions. Native ownership determines code, resource, definition, opaque HTML, and container boundaries; application tag matching cannot override those contexts. The hooks are recognizer controls, not a general document-rewriting API.

Accept complete top-level, non-nested details envelopes with one leading phrasing summary and a Markdown body. Preserve only boolean `open`; native DOM details/summary supply focus and keyboard behavior. Treat rejected wrapper syntax as visible native literal text in its surrounding paragraph, without artificial fragment breaks or swallowing ordinary following content. Native opaque HTML stays suppressed by the existing renderer, and no raw HTML reparsing or relaxed URL policy is introduced.

Separate syntax-first recognition membership from real definitions. Compile real document/body definitions through the native compiler, collect decoded definitions globally in source order with first-wins semantics, and compile summary phrasing after those definitions are available. Do not fabricate URLs, append definition strings, or parse summary text as a definition-bearing block document. Share the resulting interpretation and source ranges across rendering, streaming selection, and conversation find. Use matching explicit basic/GFM configuration; ordinary input retains native semantic parity. Positions address the original UTF-16 source, including BOM and CRLF, rather than claiming exact parity with native BOM-stripped offsets.

Only accepted frames partition compilation into disjoint native input regions. Rejected wrappers do not create ancestor/child reparsing. Fixed extension character/event passes, indexed membership, and iterative application walks enforce the per-interpretation budget in REQ-CONV-025. No fixed-point convergence loop or overlapping suffix reparsing is permitted. Count extension traversal, submitted source volume, and committed events separately from native speculation and rollback: these controls do not prove that native parser internals or downstream HAST conversion are linear or stack-safe. Each streamed update receives its own bounded interpretation, not an incremental-parser guarantee.

## Consequences

- **Positive:** One context-aware interpretation governs visible content, search, and source selection. Existing basic/GFM grammar, decoded references, and safe rendering remain authoritative. Narrow static seams avoid both compiler vendoring and runtime source manipulation.
- **Negative:** Pinned patches require review when either native package changes, including development/production export parity, borrowed EOF and write prohibition, speculative container rollback, and ordinary native behavior. Arbitrary third-party Markdown extensions are not certified by basic/GFM parity. Downstream native recursion limits remain a distinct concern; iterative application walks do not remove them.
- **Neutral:** The supported syntax is intentionally smaller than HTML: no nested disclosure language, arbitrary attribute forwarding, or HTML execution. Syntax-first shielding intentionally applies even to undefined native-complete references. Original-source BOM coordinates intentionally differ from stripped-source native coordinates. The prototype's input-size cap is an experiment constraint, not a product limit.

## References

- [Conversation UI requirements](../conversation-ui/requirements.md), REQ-CONV-025, REQ-CONV-002, REQ-CONV-019.
- [Conversation UI executive summary](../conversation-ui/executive.md) for implementation and qualification status.
- [Integration boundary](../../ui/src/components/markdown-disclosures/README.md): `interpretMarkdown`, `markdownDetailsRanges`, `markdownReferenceContext`, and `remarkDetails`.
- Static package seams: `micromark/phoenix` and `mdast-util-from-markdown/phoenix`; patches under `ui/patches/`.
