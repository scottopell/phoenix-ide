---
created: 2026-06-03
priority: p1
status: in-progress
artifact: crates/phoenix-ide/src/file_index.rs
---

Build an in-memory file index cache per conversation cwd. Bootstrap walk uses ignore::WalkBuilder, then incremental updates via notify+notify-debouncer-full. Cmd+P file search no longer pays a cold filesystem walk on every keystroke.
