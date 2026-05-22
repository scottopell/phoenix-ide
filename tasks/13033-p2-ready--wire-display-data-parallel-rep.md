`EnrichedMessage` ships the same bash `display` string in two parallel
representations on the SSE wire: once inside `display_data.bash[*].display`
and once baked into matching `tool_use` blocks at `content[*].display`. The
UI now has two equally valid sources; whichever it reads becomes the de
facto authority and the other can silently drift.

## Verified locations

- `crates/phoenix-ide/src/api/wire.rs:73-86` -- `EnrichedMessage` carries
  both `content: Value` (TS `unknown`) and `display_data: Option<Value>`
  (TS `unknown | null`), both flagged "deliberately opaque" in the module
  docs at lines 18-48.
- `crates/phoenix-ide/src/api/wire.rs:88-102` -- `From<&Message>` calls
  `enrich_content(msg)` for `content` and then ALSO clones
  `msg.display_data` into the same envelope at line 97.
- `crates/phoenix-ide/src/api/wire.rs:118-176` --
  `enrich_content`/`merge_bash_displays_into_content` walks
  `display_data.bash[*]` and `obj.insert("display", ...)` into each
  matching bash `tool_use` block in `content`. After this, the same
  string is on both wire paths.

## Why egregious vs surrounding code

This is the canonical anti-pattern AGENTS.md cites verbatim: "same image
bytes in both `display_data["data"]` (JSON blob) and `images[0].data`
(typed) -- two representations, same value, divergence risk." The
bash-display case has the same shape.

The codebase demonstrably knows the right pattern -- task 13013
(read-image-parallel-representation, done) adjudicated the
`read_image` instance of this exact smell. The wire-side instance was
not in scope for that task and remains.

Concrete divergence path: a future `MessageUpdated` broadcast (see task
08679, done) that ships fresh `display_data` while reusing a previously
enriched `content` will leave the two out of sync; the UI will see stale
strings in `content[*].display` while `display_data.bash[*].display`
holds the new value.

## Existing mitigation

The module docs (`api/wire.rs:18-48`) declare both fields "deliberately
opaque" with `#[ts(type = "unknown")]`. That documents the typing
decision but does not address the duplication -- it just hides it from
ts-rs.

## Fix direction

Option A (preferred): drop `display_data.bash[*]` from the SSE wire
envelope once the merge has baked it into `content`. The UI consumes
the merged `content`; the raw `display_data.bash` shape has no other
consumer. Smaller wire payload, single source of truth.

Option B: keep `display_data` as the source and remove the
`merge_bash_displays_into_content` mutation -- the UI walks
`display_data.bash` itself when rendering. Requires UI change.

Either way, `EnrichedMessage` should not ship overlapping bytes.
`merge_bash_displays_into_content` (wire.rs:136) is the seam.

## Related
- 13013 (read-image-parallel-representation, done -- same principle,
  different site)
- 08679 (sse-update-vs-message-split, done -- describes the
  MessageUpdated reconnect path that aggravates the divergence)
