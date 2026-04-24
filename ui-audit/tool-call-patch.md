# Widget: tool-call-patch

**Component:**
`ui/src/components/MessageComponents.tsx:99-105, 377-534 + ui/src/components/PatchFileSummary.tsx:83-113`
**Type:** Tool call **Description:** patch tool — file/op summary in header, collapsible
unified diff, clickable file list with per-file line-change counts.

## Screenshots

Stored at `ui-audit/screenshots/tool-call-patch/`.

- [x] `01-success.png` — isolated success state
- [x] `02-alt.png` — non-happy-path state (error / truncated / loading — mark n/a if no
  material alt)
- [x] `03-in-context.png` — widget in a real conversation stack (5–10 surrounding
  widgets)

## Scores

| Dimension | Score (1–5) | Notes |
| --- | --- | --- |
| Information density | 3 | Header summary is `path: op` (`tasks/24681-p2-ready--fix-file-autocomplete.md: modify`); the word `modify` is low-signal — the widget already lives under a tool named `patch`. `Modified files:` subtitle + per-file chip duplicates the same path a second time. |
| State legibility | 3 | `✓`/`✗` badge works for success/fail; in `02-alt.png` the failed patch shows `Could've 'repair' missing file 'operation'` inline and the `✗` icon, but the error message itself is rendered in the plain output body with no chrome differentiation — two failures in a row read as a uniform grey stack. |
| Consistency with shared widget grammar | 3 | Header + status badge match the shared grammar, but the `PatchFileSummary` block appended below is a second, differently-styled widget (its own “Modified files:” label, `FileCode` lucide icon, `ChevronRight`) — different iconography from the project’s `✓ + ✗ …` grammar, different spacing from the parent tool-block. |
| Scannability in context | 3 | In `03-in-context.png` two consecutive failed patches are nearly identical visually; without reading the path strings you can’t tell them apart. The `patch` label + filename + “modify” keyword is the only differentiator and is buried among neighboring tool-blocks. |
| Fidelity | 4 | Diff is rendered when `display_data.diff` is present; file-click summary counts lines per file faithfully (from `extractFileChanges`). One gap: when the diff is absent (error path) the header still says `path: modify` as if the operation succeeded — no visual cue that nothing was actually modified. |
| **Total** | **16 / 25** |  |

## Issues

- [density] Header text `tasks/…: modify` encodes `operation: modify` as a word; for the
  default `modify` case this is pure noise — only `delete`/`rewrite`/unusual ops carry
  signal. Source: `MessageComponents.tsx:104`.
- [density] The full file path appears twice when a diff is present: once in the header
  `tool-block-input` and once in the `PatchFileSummary` row below.
  At 2 patches per agent turn this is 4 rows of the same string.
- [state-legibility] On patch failure the error text is just bash-style stdout inside
  the output pane; there’s no “the file was not modified” framing, so a reader who
  didn’t notice the `✗` would have to parse `Couldn't 'repair' missing file 'operation'`
  themselves.
- [consistency] `PatchFileSummary` introduces `FileCode` and `ChevronRight` icons from
  lucide, while the rest of the tool-block uses hand-rolled SVG
  `CheckIcon`/`XIcon`/`ChevronDown` — two icon vocabularies in one component.
- [consistency] `"Modified files:"` label with trailing colon is the only tool-block
  sub-section with a prose label; every other section uses a sigil or nothing.
- [fidelity] `patches?.[0]?.operation` in `formatToolInput` uses only the first
  operation’s verb even when `count > 1`; for mixed `modify+delete` batches the header
  lies about what the call is doing (says `N patches` but loses the op breakdown).
- [density] When patches fail before applying, the widget still runs
  `containsUnifiedDiff` against the error text; in practice this means error cases
  correctly skip the summary, but the code path relies on the error text never
  containing `@@` — fragile.

## Recommendations

-----

Audited by: Scott
Notes: recommendations agreed. icons should be updated to be SVG, hiding header
when patchfilesummary shows is good. operation verb in the header summary is
good. Drop 'Modified files:' from PatchFileSummary is good.

for very simple edits, the PatchFileSummary would be more useful to directly
show the patch inline rather than require an expansion. Maybe when count==1 and
lines-modified <= 3 we can show it inline

-----

- Replace header `path: operation` with a compact form like
  `+12 −3 tasks/…/fix-file-autocomplete.md`, i.e. change counts first then path.
  For multi-file patches: `+24 −8 3 files` and push the list to `PatchFileSummary`. Puts
  signal (magnitude of change) left, path (already repeated below) right.
- Hide the header `tool-block-input` entirely when `PatchFileSummary` is going to render
  below — they’re redundant; keep only the `patch ✓` tool name row + the clickable file
  list.
- Swap `PatchFileSummary`’s `FileCode`/`ChevronRight` lucide icons for the project’s
  existing `CheckIcon`-style SVGs or drop icons entirely; use a `›` text chevron to
  match elsewhere.
- On failure, tint the output pane and prepend a one-line “patch not applied” banner
  inside the output area, so error-vs-success is readable without hunting for the corner
  badge — important because patches often fail in pairs (see `02-alt.png`).
- When `count > 1`, show each operation verb in the header summary (e.g.
  `3 patches: 2 modify, 1 delete`) or drop the operation summary for multi-patch calls
  and let `PatchFileSummary` carry it.
- Drop the literal `"Modified files:"` header row; the lucide-file icons + line-count
  chips already signal “this is a file list”.
