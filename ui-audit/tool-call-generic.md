# Widget: tool-call-generic

**Component:** `ui/src/components/MessageComponents.tsx:125-129, 377-534` **Type:** Tool
call (fallback) **Description:** Catch-all for unspecialized tools: browser_*, mcp,
read_file, search, ask_user_question, propose_task, terminal_command_history,
terminal_last_command, skill.
Renders raw JSON input with tool-name header and standard output section.

**Frequency notes.** Of the 9 tools that fall through this widget, `read_file` and
`search` are high-frequency (many instances per typical conversation).
Screenshot selection should prioritize those — that’s where score × frequency pain
actually lives.
Browser, mcp, terminal_*, skill, ask_user_question, propose_task are rare
by comparison.

**Cross-references (low priority):** `ask_user_question` has a live modal counterpart in
`question-panel.md`, and `propose_task` in `task-approval-reader.md`. Noted for
completeness, not because they drive scoring for this widget.

**Scope of this eval:** scoring against `read_file` (the high-frequency consumer) as
shown in screenshots.
The other 8 tools that fall through this fallback path were not screenshot-sampled for
this pilot; their rendering shares the same JSON-blob input surface so most issues
generalize, but widgets like `ask_user_question`/`propose_task` with rich input shapes
would likely score worse than read_file on density if captured.

## Screenshots

Stored at `ui-audit/screenshots/tool-call-generic/`.

- [x] `01-success.png` — isolated success state
- [x] `02-alt.png` — non-happy-path state (error / truncated / loading — mark n/a if no
  material alt)
- [x] `03-in-context.png` — widget in a real conversation stack (5–10 surrounding
  widgets)

## Scores

| Dimension | Score (1–5) | Notes |
| --- | --- | --- |
| Information density | 1 | `read_file` with a single `path` argument renders as a 3-line pretty-printed JSON object `{ "path": "/Users/…/design.md" }` plus braces — the argument is one scalar but the widget pays multi-line JSON tax every time. In `01-success.png` this is by far the tallest tool-block shown. |
| State legibility | 3 | Shared `✓`/`✗` badge works; `02-alt.png` shows successful reads where even the error hypothetical would be indistinguishable from success in the body because the output is also raw content. No special “file not found” framing — a missing-file response is just text in the output pane. |
| Consistency with shared widget grammar | 4 | Structurally identical to bash/patch/think (same header, same input box, same output). The inconsistency is semantic: the input box hosts formatted JSON instead of a natural key (path, query, url) like every specialized tool. Layout conforms; content diverges. |
| Scannability in context | 2 | In `01-success.png` / `03-in-context.png`, three stacked `read_file` blocks each occupy ~5 lines of input for a single-path operation, then their output (file contents, ~100+ lines) is shown expanded via the `<200 char` threshold OR collapsed with 3-line preview. When 5 read_files stack in a row (common exploratory pattern), the conversation becomes unreadable. |
| Fidelity | 4 | Inherits the shared output pipeline: truncation at 5000 chars is explicit, line count shown, `(empty)` placeholder. Copy-output works. Nothing hidden — the widget is verbose to a fault rather than lossy. |
| **Total** | **14 / 25** |  |

## Issues

- [density] `formatToolInput` default case runs `JSON.stringify(input, null, 2)` — the
  pretty-print adds newlines between every key even for one-field objects.
  For `read_file`, the input is effectively always one path, but always rendered as
  `{\n "path": "..."\n}`.
- [density] Generic tool-blocks do not benefit from the `$ ` prefix / `path: op` summary
  that specialized tools get; the tool name in the header is the only semantic hint, so
  reading “what did read_file just read” requires parsing JSON.
- [scannability] In `03-in-context.png` the stack of generic blocks is visually uniform
  — every read_file looks the same height and shape, so finding “the read_file that got
  the interesting result” is impossible at skim speed.
- [state-legibility] There’s no differentiated error path for generic tools.
  A `read_file` on a missing path gets the same visual treatment as one that returned
  5KB of content; the only signal is the `✗` badge plus whatever error text the backend
  chose.
- [consistency] `read_file` is a specialized, high-frequency tool being served by a
  catch-all path — it has a dedicated spec (`specs/read_file/`) but no dedicated
  renderer. It’s an inventory bug more than a widget bug, but the audit surfaces it: this
  is the “one-fix-helps-many” target flagged in `INVENTORY.md`.
- [fidelity — browser/mcp not sampled] Tools with structured inputs (e.g.
  `browser_click { selector, text }`, `mcp { server, tool, params }`) would render their
  entire param object as JSON — likely even worse density than read_file but not
  captured in this pass.

## Recommendations

-----

Audited by: Scott
Notes: recommendations agreed. read_file needs to be a dddicated widget.

agree with rendering input as single-line JSON when small enough.

Agree with dedicated renderers for browser-*, search, and ask_user_question.
Each has a solid identity.

error tint agreed as previously suggested.

-----

- Promote `read_file` out of the fallback: add a dedicated `case 'read_file':` in
  `formatToolInput` returning `path` (or `path:line-range` if offset/limit are set) as a
  single-line display.
  Matches the `$ cmd` / `path: op` pattern used by other specialized tools.
  This one change affects the highest-frequency consumer of the fallback.
- For the remaining fallback tools, render input as single-line JSON
  (`JSON.stringify(input)` without the `null, 2`) when the payload is small enough to
  fit, falling back to pretty-print only above a threshold.
  Cuts default height from ~4 lines to 1.
- When pretty-printing is necessary, elide outer braces for the default case — the tool
  name header already says “this is parameters” and `{ ... }` adds no information.
- Add dedicated renderers for `browser_*` (URL + action), `search` (query), and
  `ask_user_question` (question text) in the same pattern as bash/patch/think.
  The spec directory layout suggests each has enough identity to warrant a 3-line `case`
  arm.
- Consider a shared “collapse input when longer than N lines” affordance that applies to
  all tool-blocks — mirrors the output-collapse already present and would help both the
  fallback and any verbose specialized inputs (think, multi-patch, keyword_search with
  many terms).
- For error rendering (applies to all tool-blocks but most felt here), tint the output
  pane on `isError` and render the first line of the error bold — the `✗` badge alone is
  not enough when five read_files stack.
