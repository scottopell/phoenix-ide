# AskUserQuestion interaction fixtures

The scenarios render the real `QuestionPanel`, intercept response/dismissal API
methods, and display submitted arguments. The product-layout story also renders
the actual ProductConversationPage with its deterministic API fixture. No model,
database, or running Phoenix backend is required.

Run `LADLE_PORT=61137 ./dev.py qa ask-user-question` from the repository root.
Set `PLAYWRIGHT_BROWSER=webkit` to run the same journey in WebKit. Nine scenarios
run at eight viewport sizes, including the 839/840 column boundary and 320 px
usable height. Output defaults to `ui/qa-artifacts/ask-user-question/`.

The capture command asserts native keyboard behavior, retained custom drafts,
collapsed notes, selected previews, stationary choices, 44 px navigation targets,
long-preview disclosure, expanded footer placement, modal isolation, and viewport
bounds before taking screenshots. For manual interaction, run `pnpm ladle` in
`ui/` and open the Ask user question stories. Reload resets each scenario.

The proven-rejection fixture fails each submission without mutation, allowing
continued editing. Unknown outcomes, request replacement, and late callbacks are
covered by QuestionPanel tests; database/runtime tests cover actual consumption.

The initial bug hunt and all corrections are summarized in
`docs/proposals/ask-user-question-validation.md`. Fixture checks do not establish
physical software-keyboard or screen-reader usability.
