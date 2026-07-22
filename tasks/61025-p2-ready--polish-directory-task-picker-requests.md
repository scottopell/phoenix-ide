Fix request and loading-state paper cuts in the new-conversation directory/task/branch pickers.

Consolidate duplicate task loading into one guarded ensureTasksLoaded path; distinguish not-started, loading, empty, and failed task states; avoid showing loading during directory/branch debounce before a request starts; and replace the directory suggestion blur timer race with deterministic pointer/focus handling. Preserve keyboard navigation and existing API contracts. Add interaction tests covering one request per action, rapid typing, suggestion selection during blur, and retryable error states.

Frontend-only. No workflow, persistence, or durable API changes.
