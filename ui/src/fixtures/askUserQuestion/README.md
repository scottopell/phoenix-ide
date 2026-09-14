# AskUserQuestion interaction fixtures

These scenarios render the real `QuestionPanel`, intercept only its response
and dismissal API methods, and display submitted arguments in a fixture
transcript. They require no model, database, or running Phoenix backend.

Run `LADLE_PORT=61137 ./dev.py qa ask-user-question` from the repository root
to capture all six scenarios at desktop and mobile sizes. Output is under
`ui/qa-artifacts/ask-user-question/`.

For interaction, run `pnpm ladle` from `ui/` and open the Ask user question
stories. The scenarios cover ordinary choices, long descriptions with mixed
preview availability, three-question navigation, multi-select, failed response,
and read-only presentation. Submission replaces the panel with captured answer
arguments; reload the story to reset it. The failure fixture deliberately fails
every submission so retained state can be inspected.

The September 13, 2026 bug hunt found custom multi-select answer loss, stale
previews, offscreen keyboard focus, missing ordinary-question notes, unnamed
choice controls, and inefficient long-option layout. Reproduction steps and
acceptance criteria are tracked in task 10005. Local screenshot/video evidence
is in `ui/dogfood-output/report.md` in the bug-hunt worktree.

These fixtures validate the component-to-API boundary. They do not prove runtime
acceptance, SSE/reconnect behavior, full conversation-shell layout, or mobile
software-keyboard behavior.
