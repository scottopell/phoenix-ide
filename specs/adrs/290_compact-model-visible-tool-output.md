# ADR-290: Compact model-visible tool output, separate from typed displays

- **Status:** Accepted
- **Date:** 2026-11-24
- **Affects:** REQ-BASH-002, REQ-BASH-004, REQ-BASH-017, REQ-RQD-010

## Context

Several tools sent the model a JSON encoding built for another consumer.
Bash sent every output line as `{"offset":N,"bytes":"…"}`, adding about half
again the size of the raw output, and echoed the command of the same `run`
call (a heredoc command doubled in context). `query_database` sent each cell
in the tagged inter-instance wire form, pretty-printed on the local route: a
50-row result was about 23 KB where native values need about 6 KB. Coordinator
reference/send results and browser eval, console and profiling results were
pretty-printed. All of these stay in context until stale-result clearing or
compaction.

## Options considered

1. **Change the wire types.** Smaller everywhere, but the bash wire is the UI
   display persisted with transcripts, and the query cell wire is the
   cross-instance contract (REQ-RQD-008).
2. **Keep the typed wire for its consumers; give the model a compact view.**

## Decision

Option 2, approved by the project owner including the REQ-BASH changes.
`BashResponse::model_text` lists line texts (offset = `start_offset + i`) and
omits `cmd` for `run` results; peek/wait/kill keep it, and the UI display is
unchanged. The UI reads `lines` through `bashResultLineTexts`, which accepts
both encodings so persisted transcripts still render.
`CoordinatorQueryResult::model_view` renders native cells, tagging only
non-finite reals and blob summaries; the peer wire is unchanged. Remaining
pretty-printed model JSON is compact.

## Consequences

- **Positive:** bash results lose about a third of their size on typical
  output; query results shrink about 4×; local and remote query output share
  one shape.
- **Negative:** the model text and the display value of a bash result now
  differ, so anything parsing the model text must use the line-text helper.
  Browser eval and profiling results show as one-line JSON in the UI.
- **Neutral:** stored transcripts keep the encoding they were recorded with.

## References

- `BashResponse::model_text`, `BashCommandEcho`, `bashResultLineTexts`
- `CoordinatorQueryResult::model_view`
