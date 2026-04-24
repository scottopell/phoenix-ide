# Widget: tool-call-think

**Component:** `ui/src/components/MessageComponents.tsx:95-97, 377-534` **Type:** Tool
call **Description:** think tool — cleaned thought text (XML artifacts stripped), no
output section.

## Screenshots

Stored at `ui-audit/screenshots/tool-call-think/`.

- [x] `01-success.png` — isolated success state
- [ ] `02-alt.png` (n/a — no material alt state; think produces no observable failure,
  no truncation, no output section to collapse)
- [x] `03-in-context.png` — widget in a real conversation stack (5–10 surrounding
  widgets)

## Scores

| Dimension | Score (1–5) | Notes |
| --- | --- | --- |
| Information density | 2 | Renders the entire thought as multiline prose inside the `tool-block-input` box with no fold/summary — in `01-success.png` a single `think` call occupies more vertical space than 4 adjacent `bash` calls combined in `03-in-context.png`. |
| State legibility | 4 | Think has effectively one state ("a thought happened"); the tool name `think` + the prose reads unambiguously. `✓` badge only appears if a tool result arrives, which for think is consistent with the “no output” model. |
| Consistency with shared widget grammar | 4 | Uses the shared header + input-box + copy button scaffold; deviation is “no output section ever”, which is semantically correct. Minor: the input box, designed for code/commands, hosts wrapped natural-language prose — typography (likely monospace) makes paragraphs hard to read. |
| Scannability in context | 2 | In `03-in-context.png` the think block dwarfs every other tool-block; stacking 3 think calls in a turn destroys visual rank. No collapse, no preview, no “N lines” chip — the whole thought is always expanded. |
| Fidelity | 4 | `cleanThoughts` strips `<thinking>` wrappers and truncates at `</thinking>`; the comment in source (`MessageComponents.tsx:72-85`) documents this. Loss is deliberate and specific. Gap: truncation happens silently — a user who wondered why the thought cuts mid-sentence gets no affordance to view the raw text. |
| **Total** | **16 / 25** |  |

## Issues

- [density] Think text is never collapsed.
  Unlike bash/read_file which honor `OUTPUT_AUTO_EXPAND_THRESHOLD = 200` on their
  *output*, the think tool’s *input* (which is the whole payload) has no such threshold;
  a 40-line reasoning chain becomes 40 lines of the conversation.
- [scannability] In `03-in-context.png` the think block visually outranks the bash calls
  around it purely by size, even though the user is usually more interested in actions
  than narration. Size encodes importance inverted from user priority.
- [consistency] Every sibling tool-block (bash, patch, read_file, keyword_search) has
  both an input and an output/result section; think has only input.
  This is the right semantic but breaks the “every tool-block has two halves” reading
  rhythm.
- [fidelity] `cleanThoughts` silently drops everything after `</thinking>` with no
  indicator that post-thinking content was truncated.
  `MessageComponents.tsx:80-83` comments this as model-narration artifact, but there’s
  no UI tell that the displayed thought is shorter than the stored one.
- [density] The input box styling is tuned for monospace (bash, code) — wrapped prose
  paragraphs in monospace are harder to read than in the proportional font used by the
  surrounding agent-text-block.

## Recommendations

-----

Audited by: Scott
Notes: recommendations agreed. apply same 200-char threshold is good. Agree with
natural language typography. `cleanThoughts` discard is fine as-is, no need to
change this.

We should not keep tool-block shell if possible, thinking should be expandable
and subtle by default.

-----

- Apply the same 200-char threshold used for output to think input: show first 3 lines
  with “+N more lines” preview, click to expand.
  Think stops dominating the scroll and matches the rhythm of long bash output.
- Render the thought in the same prose typography as `agent-text-block` (proportional
  font, normal paragraph width) instead of the monospace `tool-block-input` — it’s
  natural language, not a command.
- Consider demoting `think` from “tool-block with header” to a lighter inline treatment
  (e.g. an italic bracketed aside) — it has no output, no status, no failure mode; it’s
  doing less work than the tool-block chrome implies.
- When `cleanThoughts` discards text after `</thinking>`, surface a subtle “…” or
  “[model narration stripped]” marker with a copy-raw affordance, so the truth about
  what was dropped is visible.
- If keeping the tool-block shell, skip rendering the `✓` status slot for think entirely
  (there’s never a result to be successful about) — current code already conditions on
  `hasOutput`, but worth explicitly documenting think’s always-no-status behavior.
