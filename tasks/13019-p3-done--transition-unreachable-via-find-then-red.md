Two `unreachable!()`s in the core transition fn enforce an invariant the types don't guarantee: a tool is located by `.find(|t| matches!(t.input, ToolInput::X(_)))`, then the same value is re-destructured with `if let ToolInput::X(ref input) = tool.input` and the `else` path is `unreachable!`.

## Verified locations
- crates/phoenix-ide/src/state_machine/transition.rs:1639-1742 — propose_task: `.find(|t| matches!(t.input, ToolInput::ProposeTask(_)))` then `if let ToolInput::ProposeTask(ref input) = tool.input { ... }` and `unreachable!("propose_task_tool matched but input was not ProposeTask")` at :1742.
- crates/phoenix-ide/src/state_machine/transition.rs:1746-1796 — ask_user_question: identical shape, `unreachable!("ask_question_tool matched but input was not AskUserQuestion")` at :1796.

## Why it matters (correct-by-construction)
The compiler cannot see that the `find` predicate and the re-destructure are linked. A refactor of the predicate turns a logic bug into a runtime panic in the conversation runtime. The invalid state ("matched X but input isn't X") should be unrepresentable, not panic-guarded.

## Fix direction
Replace the `.find(...)` + inner `if let` + `unreachable!` with `.find_map(|t| match &t.input { ToolInput::ProposeTask(input) => Some((t, input)), _ => None })`, binding `(tool, input)` once so the `unreachable!` becomes literally unwriteable. Apply the same to the ask_user_question block. NOTE: this requires dedenting the ~60-line body of each block by one level — mechanically simple but not a one-liner, which is why it is its own task rather than folded into the audit PR (avoids a large blind reindent in the hottest transition fn).

## Out of scope / related
- transition.rs:2152 (`_ => unreachable!("is_terminal_tool returned true for non-terminal tool")`) is the same family (invariant split across `is_terminal_tool` and the match); the 668/671/738/1817/2164 `unreachable!`s are state/event-shape guards, a different shape — handle separately if at all.
- Sibling audit tasks: 13016, 13018.
